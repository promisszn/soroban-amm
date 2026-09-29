//! V2 → V3 AMM Migration Contract (Issue #266)
//!
//! Atomically migrates a liquidity position from a V2 constant-product pool
//! to a V3 concentrated-liquidity pool in a single transaction.
//!
//! Flow:
//!   1. LP authorizes the migration call and its nested pool calls.
//!   2. LP calls `migrate` with their V2 LP share amount and desired V3 range.
//!   3. The V2 pool burns the shares and returns token_a + token_b to the LP.
//!   4. The CL pool pulls only the amounts needed for the selected range
//!      directly from the LP and records the position under the LP's address.
//!   5. Any leftover tokens remain in the LP's wallet.

#![no_std]

use pool_interfaces::ConcentratedLiquidityClient;
use soroban_amm_sdk::emit_versioned_event;
use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, Address, Env,
};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MigrationError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    ZeroShares = 4,
    InvalidRange = 5,
    SlippageExceeded = 6,
    MigrationFailed = 7,
    TokenMismatch = 8,
}

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    V2Pool,
    V3Pool,
}

const MIN_TTL: u32 = 172_800;
const BUMP_TO: u32 = 518_400;
const MIN_TICK: i32 = -887_272;
const MAX_TICK: i32 = 887_272;

// ── External interfaces ───────────────────────────────────────────────────────

/// Minimal V2 AMM interface needed for migration.
#[contractclient(name = "V2PoolClient")]
pub trait V2PoolInterface {
    fn remove_liquidity(
        env: Env,
        provider: Address,
        shares: i128,
        min_a: i128,
        min_b: i128,
        deadline: u64,
    ) -> Result<(i128, i128), soroban_sdk::Error>;

    fn get_info(env: Env) -> V2PoolInfo;
}

#[contracttype]
#[derive(Clone)]
pub struct V2PoolInfo {
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

// ── Migration result ──────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationResult {
    /// Amount of the V3 pool's token_a deposited into the CL position.
    pub deposited_a: i128,
    /// Amount of the V3 pool's token_b deposited into the CL position.
    pub deposited_b: i128,
    /// Amount of the V3 pool's token_a that remained with the LP.
    pub leftover_a: i128,
    /// Amount of the V3 pool's token_b that remained with the LP.
    pub leftover_b: i128,
    /// Final lower tick used for the V3 range.
    pub tick_lower: i32,
    /// Final upper tick used for the V3 range.
    pub tick_upper: i32,
    /// Receipt NFT minted by the CL pool, if position NFTs are configured.
    pub position_token_id: Option<u64>,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct MigrationContract;

fn extend_ttl(env: &Env) {
    env.storage().instance().extend_ttl(MIN_TTL, BUMP_TO);
}

#[contractimpl]
impl MigrationContract {
    // ── Setup ─────────────────────────────────────────────────────────────────

    /// Initialize the migration helper.
    ///
    /// # Parameters
    /// - `admin`   – Contract administrator.
    /// - `v2_pool` – Address of the V2 constant-product AMM pool.
    /// - `v3_pool` – Address of the V3 concentrated-liquidity pool.
    pub fn initialize(
        env: Env,
        admin: Address,
        v2_pool: Address,
        v3_pool: Address,
    ) -> Result<(), MigrationError> {
        extend_ttl(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(MigrationError::AlreadyInitialized);
        }

        admin.require_auth();

        // Verify V2 and V3 pools trade the same token pair.
        let v2_client = V2PoolClient::new(&env, &v2_pool);
        let v2_info = v2_client.get_info();

        let v3_client = ConcentratedLiquidityClient::new(&env, &v3_pool);
        let (v3_token_a, v3_token_b) = v3_client.get_tokens();

        if !((v2_info.token_a == v3_token_a && v2_info.token_b == v3_token_b)
            || (v2_info.token_a == v3_token_b && v2_info.token_b == v3_token_a))
        {
            return Err(MigrationError::TokenMismatch);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::V2Pool, &v2_pool);
        env.storage().instance().set(&DataKey::V3Pool, &v3_pool);

        Ok(())
    }

    // ── Migration ─────────────────────────────────────────────────────────────

    /// Migrate a V2 LP position to V3 in a single atomic transaction.
    ///
    /// # Parameters
    /// - `provider`       – LP address; must authorise this call.
    /// - `v2_shares`      – Number of V2 LP tokens to burn.
    /// - `min_a`          – Minimum token_a to receive from V2 withdrawal (slippage).
    /// - `min_b`          – Minimum token_b to receive from V2 withdrawal (slippage).
    /// - `tick_lower`     – Desired lower tick for the V3 range.
    ///                      Pass `i32::MIN` to auto-compute an optimal range.
    /// - `tick_upper`     – Desired upper tick for the V3 range.
    ///                      Pass `i32::MAX` to auto-compute an optimal range.
    /// - `range_width_ticks` – Half-width of the auto-computed range (ignored when
    ///                         explicit ticks are provided).
    /// - `min_deposit_a`  – Minimum V3 token_a deposited (slippage guard).
    /// - `min_deposit_b`  – Minimum V3 token_b deposited (slippage guard).
    /// - `deadline`       – Latest ledger timestamp at which this call is valid.
    ///
    /// # Returns
    /// A [`MigrationResult`] describing what was deposited and left with the LP,
    /// the final range, and the optional V3 position NFT ID.
    #[allow(clippy::too_many_arguments)]
    pub fn migrate(
        env: Env,
        provider: Address,
        v2_shares: i128,
        min_a: i128,
        min_b: i128,
        tick_lower: i32,
        tick_upper: i32,
        range_width_ticks: i32,
        min_deposit_a: i128,
        min_deposit_b: i128,
        deadline: u64,
    ) -> Result<MigrationResult, MigrationError> {
        extend_ttl(&env);
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(MigrationError::NotInitialized);
        }

        if v2_shares <= 0 {
            return Err(MigrationError::ZeroShares);
        }

        provider.require_auth();

        let v2_pool: Address = env.storage().instance().get(&DataKey::V2Pool).unwrap();

        let v3_pool: Address = env.storage().instance().get(&DataKey::V3Pool).unwrap();

        // ── Step 0: verify V2 and V3 pools trade the same pair ──────────────
        let v2_client = V2PoolClient::new(&env, &v2_pool);
        let pool_info = v2_client.get_info();

        let token_a = pool_info.token_a.clone();
        let token_b = pool_info.token_b.clone();

        let v3_client = ConcentratedLiquidityClient::new(&env, &v3_pool);
        let (v3_token_a, v3_token_b) = v3_client.get_tokens();

        let reversed = if token_a == v3_token_a && token_b == v3_token_b {
            false
        } else if token_a == v3_token_b && token_b == v3_token_a {
            true
        } else {
            return Err(MigrationError::TokenMismatch);
        };

        // Validate the range before burning shares or moving any funds.
        let (final_tick_lower, final_tick_upper) =
            Self::compute_range(&v3_client, tick_lower, tick_upper, range_width_ticks)?;

        // The V2 pool returns the withdrawn assets directly to the LP.
        let (received_a, received_b) =
            v2_client.remove_liquidity(&provider, &v2_shares, &min_a, &min_b, &deadline);

        // Map the withdrawal into the CL pool's canonical token order.
        let (desired_a, desired_b) = if reversed {
            (received_b, received_a)
        } else {
            (received_a, received_b)
        };

        // Mint under the LP's address. The signed auth tree authorizes the
        // nested V2 withdrawal, CL mint, and token transfers from the LP.
        let (deposited_a, deposited_b) = v3_client.mint_position(
            &provider,
            &final_tick_lower,
            &final_tick_upper,
            &desired_a,
            &desired_b,
            &min_deposit_a,
            &min_deposit_b,
            &deadline,
        );

        let leftover_a = desired_a
            .checked_sub(deposited_a)
            .ok_or(MigrationError::MigrationFailed)?;
        let leftover_b = desired_b
            .checked_sub(deposited_b)
            .ok_or(MigrationError::MigrationFailed)?;
        let position_token_id =
            v3_client.position_token_id(&provider, &final_tick_lower, &final_tick_upper);

        emit_versioned_event!(
            env,
            (soroban_sdk::Symbol::new(&env, "migrated"), provider.clone()),
            (
                v2_shares,
                deposited_a,
                deposited_b,
                position_token_id,
                leftover_a,
                leftover_b,
            )
        );

        Ok(MigrationResult {
            deposited_a,
            deposited_b,
            leftover_a,
            leftover_b,
            tick_lower: final_tick_lower,
            tick_upper: final_tick_upper,
            position_token_id,
        })
    }

    // ── Read-only helpers ─────────────────────────────────────────────────────

    /// Preview the optimal V3 tick range for a given V2 position without executing.
    ///
    /// Useful for off-chain UIs to show the user what range they'll get before
    /// they sign the migration transaction.
    pub fn preview_range(
        env: Env,
        tick_lower: i32,
        tick_upper: i32,
        range_width_ticks: i32,
    ) -> Result<(i32, i32), MigrationError> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(MigrationError::NotInitialized);
        }

        let v3_pool: Address = env.storage().instance().get(&DataKey::V3Pool).unwrap();

        let v3_client = ConcentratedLiquidityClient::new(&env, &v3_pool);

        Self::compute_range(&v3_client, tick_lower, tick_upper, range_width_ticks)
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    /// Compute the final [tick_lower, tick_upper] for the V3 deposit.
    ///
    /// `tick_lower` and `tick_upper` are independently opt-in to
    /// auto-computation, matching `migrate`'s doc comment: passing
    /// `i32::MIN` for `tick_lower` auto-computes only the lower bound, and
    /// passing `i32::MAX` for `tick_upper` auto-computes only the upper
    /// bound. An explicit value for either bound is always kept as-is —
    /// it is never silently discarded, even when the other bound is a
    /// sentinel.
    fn compute_range(
        v3_client: &ConcentratedLiquidityClient,
        tick_lower: i32,
        tick_upper: i32,
        range_width_ticks: i32,
    ) -> Result<(i32, i32), MigrationError> {
        let lower_auto = tick_lower == i32::MIN;
        let upper_auto = tick_upper == i32::MAX;
        let state = v3_client.get_pool_state();
        let tick_spacing = state.tick_spacing;

        if tick_spacing <= 0 {
            return Err(MigrationError::InvalidRange);
        }

        if (!lower_auto && !Self::is_valid_explicit_tick(tick_lower, tick_spacing))
            || (!upper_auto && !Self::is_valid_explicit_tick(tick_upper, tick_spacing))
        {
            return Err(MigrationError::InvalidRange);
        }

        if (lower_auto || upper_auto) && range_width_ticks <= 0 {
            return Err(MigrationError::InvalidRange);
        }

        let spacing = i64::from(tick_spacing);
        let min_usable_tick = Self::align_up(i64::from(MIN_TICK), spacing);
        let max_usable_tick = Self::align_down(i64::from(MAX_TICK), spacing);
        let current_tick = i64::from(state.current_tick);
        let width = i64::from(range_width_ticks);

        // Only compute the bounds that were explicitly marked with sentinels.
        // An explicit lower/upper bound must never be silently overwritten.
        let lower = if lower_auto {
            Self::align_down(current_tick - width, spacing).clamp(min_usable_tick, max_usable_tick)
                as i32
        } else {
            tick_lower
        };

        let upper = if upper_auto {
            Self::align_up(current_tick + width, spacing).clamp(min_usable_tick, max_usable_tick)
                as i32
        } else {
            tick_upper
        };

        if lower >= upper {
            return Err(MigrationError::InvalidRange);
        }

        Ok((lower, upper))
    }

    fn is_valid_explicit_tick(tick: i32, tick_spacing: i32) -> bool {
        (MIN_TICK..=MAX_TICK).contains(&tick) && tick % tick_spacing == 0
    }

    fn align_down(tick: i64, tick_spacing: i64) -> i64 {
        tick.div_euclid(tick_spacing) * tick_spacing
    }

    fn align_up(tick: i64, tick_spacing: i64) -> i64 {
        let quotient = tick.div_euclid(tick_spacing);
        if tick.rem_euclid(tick_spacing) == 0 {
            quotient * tick_spacing
        } else {
            (quotient + 1) * tick_spacing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amm::{AmmPool, AmmPoolClient};
    extern crate std;
    use pool_interfaces::PoolState;
    use soroban_sdk::testutils::{Address as _, Events, Ledger as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::token::{StellarAssetClient, TokenClient};
    use soroban_sdk::{IntoVal, String, Symbol, TryFromVal};
    use token::{LpToken, LpTokenClient};

    /// Minimal CL state stub used only by the range helper tests.
    #[contract]
    struct RangePool;

    #[contractimpl]
    impl RangePool {
        pub fn get_pool_state(_env: Env) -> PoolState {
            PoolState {
                sqrt_price: 0,
                current_tick: 1_000,
                active_liquidity: 0,
                tick_spacing: 1,
            }
        }

        pub fn get_tokens(env: Env) -> (Address, Address) {
            (
                env.storage().instance().get(&0u32).unwrap(),
                env.storage().instance().get(&1u32).unwrap(),
            )
        }

        pub fn set_tokens(env: Env, token_a: Address, token_b: Address) {
            env.storage().instance().set(&0u32, &token_a);
            env.storage().instance().set(&1u32, &token_b);
        }
    }

    /// Minimal V2 pool stub: only `get_info` is needed so `initialize`'s
    /// token-pair validation passes; `preview_range` never calls it.
    #[contract]
    struct MockV2Pool;

    #[contractimpl]
    impl MockV2Pool {
        pub fn get_info(env: Env) -> V2PoolInfo {
            let token_a: Address = env.storage().instance().get(&0u32).unwrap();
            let token_b: Address = env.storage().instance().get(&1u32).unwrap();
            V2PoolInfo {
                token_a,
                token_b,
                reserve_a: 0,
                reserve_b: 0,
                total_shares: 0,
                fee_bps: 0,
                flash_loan_fee_bps: 0,
                admin: env.current_contract_address(),
                fee_recipient: env.current_contract_address(),
                protocol_fee_bps: 0,
                lp_rebate_bps: 0,
            }
        }

        pub fn set_v2_tokens(env: Env, token_a: Address, token_b: Address) {
            env.storage().instance().set(&0u32, &token_a);
            env.storage().instance().set(&1u32, &token_b);
        }
    }

    fn setup() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let token_a = Address::generate(&env);
        let token_b = Address::generate(&env);

        let v2_pool = env.register_contract(None, MockV2Pool);
        MockV2PoolClient::new(&env, &v2_pool).set_v2_tokens(&token_a, &token_b);

        let v3_pool = env.register_contract(None, RangePool);
        RangePoolClient::new(&env, &v3_pool).set_tokens(&token_a, &token_b);

        let contract_addr = env.register_contract(None, MigrationContract);

        MigrationContractClient::new(&env, &contract_addr).initialize(&admin, &v2_pool, &v3_pool);

        (env, contract_addr)
    }

    #[test]
    fn both_explicit_ticks_are_kept_as_is() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.preview_range(&100, &200, &0);

        assert_eq!(result, (100, 200));
    }

    #[test]
    fn both_sentinels_auto_compute_symmetric_range() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.preview_range(&i32::MIN, &i32::MAX, &50);

        assert_eq!(result, (950, 1050));
    }

    /// Regression test for issue #478: an explicit `tick_lower` combined with
    /// `tick_upper = i32::MAX` must auto-compute only the upper bound and
    /// keep the caller's explicit lower bound, rather than silently
    /// discarding it in favor of a fully symmetric range.
    #[test]
    fn explicit_lower_with_auto_upper_keeps_explicit_lower() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.preview_range(&500, &i32::MAX, &50);

        assert_eq!(result, (500, 1050));
    }

    /// Mirror case: auto lower bound with an explicit upper bound.
    #[test]
    fn auto_lower_with_explicit_upper_keeps_explicit_upper() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.preview_range(&i32::MIN, &1500, &50);

        assert_eq!(result, (950, 1500));
    }

    #[test]
    fn partial_auto_range_rejects_non_positive_width() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.try_preview_range(&500, &i32::MAX, &0);

        assert_eq!(result, Err(Ok(MigrationError::InvalidRange)));
    }

    #[test]
    fn partial_auto_range_rejects_inverted_result() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        // Explicit lower (2000) ends up above the auto-computed upper
        // (current_tick + 50 = 1050); this must be rejected, not silently
        // deposited into an inverted/degenerate range.
        let result = client.try_preview_range(&2_000, &i32::MAX, &50);

        assert_eq!(result, Err(Ok(MigrationError::InvalidRange)));
    }

    #[test]
    fn explicit_ticks_reject_inverted_range() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.try_preview_range(&200, &100, &0);

        assert_eq!(result, Err(Ok(MigrationError::InvalidRange)));
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Issue #686: full migration unit-test suite against real contracts.
    //
    // The tests below register the REAL `amm` V2 pool plus real SEP-41 token
    // contracts (`StellarAssetClient`) in the test Env, so the funds-moving
    // cross-contract calls — `remove_liquidity`, `transfer`, `approve`,
    // `transfer_from`, and LpToken `mint`/`burn` — all run against deployed
    // contracts rather than hand-rolled stubs.
    //
    // A small CL-ABI harness keeps the unit tests focused; the integration suite
    // registers the real ConcentratedLiquidity contract for end-to-end coverage.
    mod test_v3_pool {
        use super::*;

        #[contract]
        pub(crate) struct TestV3Pool;

        #[contracttype]
        #[derive(Clone, Debug, PartialEq)]
        pub(crate) struct V3Position {
            pub(crate) provider: Address,
            pub(crate) tick_lower: i32,
            pub(crate) tick_upper: i32,
            pub(crate) deposited_a: i128,
            pub(crate) deposited_b: i128,
        }

        #[contracttype]
        enum V3DataKey {
            TokensA,
            TokensB,
            CurrentTick,
            NextPositionId,
            Position(u64),
            PositionId(Address, i32, i32),
        }

        #[contractimpl]
        impl TestV3Pool {
            pub fn setup(env: Env, token_a: Address, token_b: Address, initial_tick: i32) {
                env.storage().instance().set(&V3DataKey::TokensA, &token_a);
                env.storage().instance().set(&V3DataKey::TokensB, &token_b);
                env.storage()
                    .instance()
                    .set(&V3DataKey::CurrentTick, &initial_tick);
                env.storage()
                    .instance()
                    .set(&V3DataKey::NextPositionId, &0_u64);
            }

            pub fn set_current_tick(env: Env, tick: i32) {
                env.storage().instance().set(&V3DataKey::CurrentTick, &tick);
            }

            pub fn get_pool_state(env: Env) -> PoolState {
                let current_tick = env
                    .storage()
                    .instance()
                    .get(&V3DataKey::CurrentTick)
                    .unwrap_or(0);
                PoolState {
                    sqrt_price: 0,
                    current_tick,
                    active_liquidity: 0,
                    tick_spacing: 1,
                }
            }

            pub fn get_tokens(env: Env) -> (Address, Address) {
                let token_a: Address = env.storage().instance().get(&V3DataKey::TokensA).unwrap();
                let token_b: Address = env.storage().instance().get(&V3DataKey::TokensB).unwrap();
                (token_a, token_b)
            }

            pub fn position(env: Env, position_id: u64) -> V3Position {
                env.storage()
                    .instance()
                    .get(&V3DataKey::Position(position_id))
                    .unwrap()
            }

            /// Number of positions minted so far (0 before the first deposit).
            pub fn position_count(env: Env) -> i128 {
                let next: u64 = env
                    .storage()
                    .instance()
                    .get(&V3DataKey::NextPositionId)
                    .unwrap_or(0);
                i128::from(next)
            }

            /// Mirrors the real CL `mint_position` surface and direct-provider
            /// token flow while keeping the unit-test math deterministic.
            #[allow(clippy::too_many_arguments)]
            pub fn mint_position(
                env: Env,
                provider: Address,
                tick_lower: i32,
                tick_upper: i32,
                amount_a: i128,
                amount_b: i128,
                min_a: i128,
                min_b: i128,
                _deadline: u64,
            ) -> Result<(i128, i128), soroban_sdk::Error> {
                if tick_lower >= tick_upper {
                    return Err(soroban_sdk::Error::from_contract_error(1));
                }
                if amount_a < min_a || amount_b < min_b {
                    return Err(soroban_sdk::Error::from_contract_error(2));
                }
                provider.require_auth();

                let token_a: Address = env.storage().instance().get(&V3DataKey::TokensA).unwrap();
                let token_b: Address = env.storage().instance().get(&V3DataKey::TokensB).unwrap();
                let self_addr = env.current_contract_address();

                if amount_a > 0 {
                    TokenClient::new(&env, &token_a).transfer(&provider, &self_addr, &amount_a);
                }
                if amount_b > 0 {
                    TokenClient::new(&env, &token_b).transfer(&provider, &self_addr, &amount_b);
                }

                let next: u64 = env
                    .storage()
                    .instance()
                    .get(&V3DataKey::NextPositionId)
                    .unwrap_or(0);
                env.storage().instance().set(
                    &V3DataKey::Position(next),
                    &V3Position {
                        provider: provider.clone(),
                        tick_lower,
                        tick_upper,
                        deposited_a: amount_a,
                        deposited_b: amount_b,
                    },
                );
                env.storage()
                    .instance()
                    .set(&V3DataKey::NextPositionId, &(next + 1));
                env.storage().instance().set(
                    &V3DataKey::PositionId(provider, tick_lower, tick_upper),
                    &next,
                );

                Ok((amount_a, amount_b))
            }

            pub fn position_token_id(
                env: Env,
                provider: Address,
                tick_lower: i32,
                tick_upper: i32,
            ) -> Option<u64> {
                env.storage()
                    .instance()
                    .get(&V3DataKey::PositionId(provider, tick_lower, tick_upper))
            }
        }
    }

    pub(crate) use test_v3_pool::{TestV3Pool, TestV3PoolClient};

    // ── Shared fixture ─────────────────────────────────────────────────────────

    const DEADLINE: u64 = u64::MAX;

    struct Fixture<'a> {
        admin: Address,
        lp: Address,
        lp_shares: i128,
        ta: TokenClient<'a>,
        tb: TokenClient<'a>,
        ta_sac: StellarAssetClient<'a>,
        tb_sac: StellarAssetClient<'a>,
        v2_pool: Address,
        v2_lp: LpTokenClient<'a>,
        v3_pool: Address,
        v3: TestV3PoolClient<'a>,
        migration: MigrationContractClient<'a>,
        migration_addr: Address,
    }

    fn create_sac<'a>(env: &'a Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
        let c = env.register_stellar_asset_contract_v2(admin.clone());
        (
            TokenClient::new(env, &c.address()),
            StellarAssetClient::new(env, &c.address()),
        )
    }

    /// Bootstrap a full fixture around the REAL V2 AMM:
    ///   - `admin` seeds the pool with 6_000_000/6_000_000 (the first deposit
    ///     locks MINIMUM_LIQUIDITY = 1_000; admin receives 5_999_000 LP);
    ///   - `lp` mints exactly `lp_deposit_a`/`lp_deposit_b` and deposits
    ///     them, ending with reserves 7_000_000/7_000_000 (+lp_deposit_b)
    ///     and total shares 7_000_000 (+lp_deposit_b);
    ///   - the registered `TestV3Pool` harness is wired to the pair in the
    ///     `reversed` order when requested;
    ///   - the migration contract is initialized against both pools.
    fn build_fixture(
        env: &Env,
        lp_deposit_a: i128,
        lp_deposit_b: i128,
        reversed: bool,
        initial_tick: i32,
    ) -> Fixture<'_> {
        env.budget().reset_unlimited();
        env.mock_all_auths();

        let admin = Address::generate(env);
        let lp = Address::generate(env);
        let fee_recipient = Address::generate(env);

        let (ta, ta_sac) = create_sac(env, &admin);
        let (tb, tb_sac) = create_sac(env, &admin);

        // ── Real V2 AMM pool + real LpToken ──
        let v2_addr = env.register_contract(None, AmmPool);
        let v2_lp_addr = env.register_contract(None, LpToken);
        LpTokenClient::new(env, &v2_lp_addr).initialize(
            &v2_addr,
            &String::from_str(env, "V2 LP"),
            &String::from_str(env, "V2LP"),
            &7u32,
        );
        let v2 = AmmPoolClient::new(env, &v2_addr);
        v2.initialize(
            &admin,
            &ta.address,
            &tb.address,
            &v2_lp_addr,
            &30_i128,
            &fee_recipient,
            &0_i128,
        );

        ta_sac.mint(&admin, &10_000_000_i128);
        tb_sac.mint(&admin, &10_000_000_i128);
        v2.add_liquidity(&admin, &6_000_000_i128, &6_000_000_i128, &0_i128, &DEADLINE);
        assert_eq!(
            LpTokenClient::new(env, &v2_lp_addr).balance(&admin),
            5_999_000_i128,
            "6M initial shares minus the 1_000 locked MINIMUM_LIQUIDITY"
        );

        ta_sac.mint(&lp, &lp_deposit_a);
        tb_sac.mint(&lp, &lp_deposit_b);
        let lp_shares = v2.add_liquidity(&lp, &lp_deposit_a, &lp_deposit_b, &0_i128, &DEADLINE);

        // ── V3 harness ──
        let v3_addr = env.register_contract(None, TestV3Pool);
        let v3 = TestV3PoolClient::new(env, &v3_addr);
        if reversed {
            v3.setup(&tb.address, &ta.address, &initial_tick);
        } else {
            v3.setup(&ta.address, &tb.address, &initial_tick);
        }

        // ── Migration contract ──
        let migration_addr = env.register_contract(None, MigrationContract);
        let migration = MigrationContractClient::new(env, &migration_addr);
        migration.initialize(&admin, &v2_addr, &v3_addr);

        Fixture {
            admin,
            lp,
            lp_shares,
            ta,
            tb,
            ta_sac,
            tb_sac,
            v2_pool: v2_addr,
            v2_lp: LpTokenClient::new(env, &v2_lp_addr),
            v3_pool: v3_addr,
            v3,
            migration,
            migration_addr,
        }
    }

    /// Default symmetric fixture:
    /// reserves 7_000_000/7_000_000, total shares 7_000_000, `lp` holds
    /// 1_000_000 shares (a full exit withdraws exactly 1_000_000/1_000_000).
    fn fixture(env: &Env) -> Fixture<'_> {
        build_fixture(env, 1_000_000, 1_000_000, false, 0)
    }

    // ── initialize ─────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_twice_returns_already_initialized() {
        let env = Env::default();
        let f = fixture(&env);

        let second = f.migration.try_initialize(&f.admin, &f.v2_pool, &f.v3_pool);

        assert!(
            matches!(second, Err(Ok(MigrationError::AlreadyInitialized))),
            "re-initializing must be rejected"
        );

        // The pool addresses survive re-initialization attempts: preview_range
        // reads the stored V3 address and round-trips real pool state.
        assert_eq!(f.migration.preview_range(&100, &200, &0), (100, 200));
    }

    #[test]
    fn test_initialize_rejects_mismatched_v3_token_pair() {
        let env = Env::default();
        let f = fixture(&env);

        let (tc, _tc_sac) = create_sac(&env, &f.admin);
        let bad_v3 = env.register_contract(None, TestV3Pool);
        TestV3PoolClient::new(&env, &bad_v3).setup(&f.ta.address, &tc.address, &0);

        let fresh = env.register_contract(None, MigrationContract);
        let result = MigrationContractClient::new(&env, &fresh)
            .try_initialize(&f.admin, &f.v2_pool, &bad_v3);

        assert!(
            matches!(result, Err(Ok(MigrationError::TokenMismatch))),
            "a V3 pool trading a different pair must be rejected at initialize"
        );
    }

    #[test]
    fn test_migrate_before_initialize_returns_not_initialized() {
        let env = Env::default();
        let provider = Address::generate(&env);
        let contract_addr = env.register_contract(None, MigrationContract);
        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.try_migrate(
            &provider, &1_i128, &0_i128, &0_i128, &100_i32, &200_i32, &0_i32, &0_i128, &0_i128,
            &DEADLINE,
        );
        assert!(
            matches!(result, Err(Ok(MigrationError::NotInitialized))),
            "migrate before initialize must fail"
        );

        let preview = client.try_preview_range(&100, &200, &0);
        assert!(
            matches!(preview, Err(Ok(MigrationError::NotInitialized))),
            "preview_range before initialize must fail"
        );
    }

    // ── preview_range against the registered V3 harness ─────────────────────────

    #[test]
    fn test_preview_range_explicit_ticks_used_verbatim() {
        let env = Env::default();
        let f = fixture(&env);
        let client = MigrationContractClient::new(&env, &f.migration_addr);

        // Explicit bounds are kept exactly as-is regardless of the pool tick.
        assert_eq!(f.migration.preview_range(&100, &200, &9_999), (100, 200));

        // Inverted explicit bounds are rejected.
        let result = client.try_preview_range(&200, &100, &0);
        assert!(matches!(result, Err(Ok(MigrationError::InvalidRange))));
    }

    #[test]
    fn test_preview_range_sentinels_auto_bracket_current_tick() {
        let env = Env::default();
        let f = fixture(&env);

        f.v3.set_current_tick(&1_000_i32);

        assert_eq!(
            f.migration.preview_range(&i32::MIN, &i32::MAX, &500),
            (500, 1_500)
        );

        // Single-sided sentinels only replace that bound, keeping the other.
        assert_eq!(
            f.migration.preview_range(&i32::MIN, &1_500, &500),
            (500, 1_500)
        );
        assert_eq!(
            f.migration.preview_range(&500, &i32::MAX, &500),
            (500, 1_500)
        );
    }

    #[test]
    fn test_preview_range_pool_without_liquidity_still_returns_valid_range() {
        let env = Env::default();
        let f = fixture(&env);

        // No position deposited yet; the harness reports its stored current
        // tick (0) and the migration still yields a well-formed, non-empty
        // bracket rather than panicking or returning an inverted range.
        assert_eq!(
            f.migration.preview_range(&i32::MIN, &i32::MAX, &100),
            (-100, 100)
        );
    }

    #[test]
    fn test_preview_range_wider_width_auto_range_strictly_contains_narrower() {
        let env = Env::default();
        let f = fixture(&env);

        f.v3.set_current_tick(&0_i32);

        let narrow = f.migration.preview_range(&i32::MIN, &i32::MAX, &100);
        let wide = f.migration.preview_range(&i32::MIN, &i32::MAX, &1_000);

        assert_eq!(narrow, (-100, 100));
        assert_eq!(wide, (-1_000, 1_000));
        assert!(
            narrow.0 > wide.0 && narrow.1 < wide.1,
            "the wider range must strictly contain the narrower one"
        );
    }

    // ── migrate happy paths ─────────────────────────────────────────────────────

    #[test]
    fn test_full_migration_burns_all_lp_shares_and_mints_position() {
        let env = Env::default();
        let f = fixture(&env);

        assert_eq!(f.v2_lp.balance(&f.lp), 1_000_000_i128);

        let result = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // All V2 LP shares were burned by the real AMM contract.
        assert_eq!(f.v2_lp.balance(&f.lp), 0_i128);
        assert_eq!(f.v3.position_count(), 1_i128);

        // The registered V3 pool minted a real, queryable position at the
        // range auto-computed around the harness's current tick (default 0).
        let pos = f.v3.position(&result.position_token_id.unwrap());
        assert_eq!(pos.provider, f.lp);
        assert_eq!(pos.tick_lower, -500);
        assert_eq!(pos.tick_upper, 500);
        assert_eq!(pos.deposited_a, 1_000_000_i128);
        assert_eq!(pos.deposited_b, 1_000_000_i128);
    }

    #[test]
    fn test_full_migration_deposits_exact_withdrawal_amounts_with_no_dust() {
        let env = Env::default();
        let f = fixture(&env);

        // shares * reserve_x / total_shares = 1M * 7M / 7M = exactly 1M each.
        let result = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        assert_eq!(result.deposited_a, 1_000_000_i128);
        assert_eq!(result.deposited_b, 1_000_000_i128);
        assert_eq!(result.leftover_a, 0_i128);
        assert_eq!(result.leftover_b, 0_i128);

        // Both tokens physically landed in the V3 pool.
        assert_eq!(f.ta.balance(&f.v3_pool), 1_000_000_i128);
        assert_eq!(f.tb.balance(&f.v3_pool), 1_000_000_i128);
    }

    #[test]
    fn test_migration_conserves_every_token_and_does_not_touch_admin_position() {
        let env = Env::default();
        let f = fixture(&env);

        let result = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // The migrating LP ends up with nothing in their pocket...
        assert_eq!(f.ta.balance(&f.lp), 0_i128);
        assert_eq!(f.tb.balance(&f.lp), 0_i128);

        // ...the shared migration contract retains nothing...
        assert_eq!(f.ta.balance(&f.migration_addr), 0_i128);
        assert_eq!(f.tb.balance(&f.migration_addr), 0_i128);

        // ...and 100% of the withdrawn tokens sit in the V3 pool.
        assert_eq!(f.ta.balance(&f.v3_pool), result.deposited_a);
        assert_eq!(f.tb.balance(&f.v3_pool), result.deposited_b);

        // Per-token conservation: nothing was created or destroyed.
        let lp_original_a = 1_000_000_i128;
        let lp_original_b = 1_000_000_i128;
        assert_eq!(
            f.ta.balance(&f.lp) + f.ta.balance(&f.migration_addr) + f.ta.balance(&f.v3_pool),
            lp_original_a
        );
        assert_eq!(
            f.tb.balance(&f.lp) + f.tb.balance(&f.migration_addr) + f.tb.balance(&f.v3_pool),
            lp_original_b
        );

        // Admin's own V2 position was not swept up by the LP's migration.
        assert_eq!(f.v2_lp.balance(&f.admin), 5_999_000_i128);
    }

    #[test]
    fn test_partial_migration_burns_only_requested_shares() {
        let env = Env::default();
        let f = fixture(&env);

        let half = f.lp_shares / 2;
        let result = f.migration.migrate(
            &f.lp,
            &half,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // Exactly half the shares burned, the rest stays withdrawable.
        assert_eq!(f.v2_lp.balance(&f.lp), 500_000_i128);
        assert_eq!(result.deposited_a, 500_000_i128);
        assert_eq!(result.deposited_b, 500_000_i128);
        assert_eq!(f.v3.position_count(), 1_i128);

        // The LP redeems the remaining 500k shares directly on the real AMM.
        let v2 = AmmPoolClient::new(&env, &f.v2_pool);
        let (out_a, out_b) = v2.remove_liquidity(&f.lp, &500_000_i128, &0_i128, &0_i128, &DEADLINE);
        assert_eq!((out_a, out_b), (500_000_i128, 500_000_i128));

        assert_eq!(f.v2_lp.balance(&f.lp), 0_i128);
        assert_eq!(f.ta.balance(&f.lp), 500_000_i128);
        assert_eq!(f.tb.balance(&f.lp), 500_000_i128);
    }

    #[test]
    fn test_migrate_preview_ticks_match_executed_position() {
        let env = Env::default();
        let f = fixture(&env);

        f.v3.set_current_tick(&1_000_i32);

        let preview = f.migration.preview_range(&i32::MIN, &i32::MAX, &500);
        assert_eq!(preview, (500, 1_500));

        let result = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        assert_eq!((result.tick_lower, result.tick_upper), (500, 1_500));

        let pos = f.v3.position(&result.position_token_id.unwrap());
        assert_eq!((pos.tick_lower, pos.tick_upper), (500, 1_500));
    }

    #[test]
    fn test_migration_contract_never_holds_or_approves_tokens() {
        let env = Env::default();
        let f = fixture(&env);

        let _ = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // The direct LP-to-CL flow never creates migration-contract approvals.
        assert_eq!(f.ta.allowance(&f.migration_addr, &f.v3_pool), 0_i128);
        assert_eq!(f.tb.allowance(&f.migration_addr, &f.v3_pool), 0_i128);

        // The shared migration contract retained nothing from this call.
        assert_eq!(f.ta.balance(&f.migration_addr), 0_i128);
        assert_eq!(f.tb.balance(&f.migration_addr), 0_i128);

        // Behavioral proof: funds minted later to the contract are not spendable
        // by the V3 pool through any lingering allowance.
        f.ta_sac.mint(&f.migration_addr, &100_i128);
        let steal =
            f.ta.try_transfer_from(&f.v3_pool, &f.migration_addr, &f.lp, &100_i128);
        assert!(
            steal.is_err(),
            "v3_pool must not be able to move post-migration balances"
        );
        assert_eq!(f.ta.balance(&f.migration_addr), 100_i128);

        // Same guarantee for token B.
        f.tb_sac.mint(&f.migration_addr, &100_i128);
        let steal_b =
            f.tb.try_transfer_from(&f.v3_pool, &f.migration_addr, &f.lp, &100_i128);
        assert!(
            steal_b.is_err(),
            "v3_pool must not be able to move post-migration balances"
        );
        assert_eq!(f.tb.balance(&f.migration_addr), 100_i128);
    }

    // ── migrate failure paths and atomicity ─────────────────────────────────────

    #[test]
    fn test_migrate_zero_shares_is_rejected() {
        let env = Env::default();
        let f = fixture(&env);

        let result = f.migration.try_migrate(
            &f.lp,
            &0_i128,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );
        assert!(matches!(result, Err(Ok(MigrationError::ZeroShares))));

        let negative = f.migration.try_migrate(
            &f.lp,
            &-1_i128,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );
        assert!(matches!(negative, Err(Ok(MigrationError::ZeroShares))));
    }

    #[test]
    fn test_migrate_more_shares_than_owned_aborts_atomically() {
        let env = Env::default();
        let f = fixture(&env);

        let before_shares = f.v2_lp.balance(&f.lp);
        let before_ta = f.ta.balance(&f.lp);
        let before_tb = f.tb.balance(&f.lp);

        let result = f.migration.try_migrate(
            &f.lp,
            &(f.lp_shares + 1),
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );
        assert!(result.is_err(), "over-owning migration must panic/revert");

        // Nothing moved: V2 shares, the LP's tokens and the V3 pool are untouched.
        assert_eq!(f.v2_lp.balance(&f.lp), before_shares);
        assert_eq!(f.ta.balance(&f.lp), before_ta);
        assert_eq!(f.tb.balance(&f.lp), before_tb);
        assert_eq!(f.ta.balance(&f.migration_addr), 0_i128);
        assert_eq!(f.tb.balance(&f.migration_addr), 0_i128);
        assert_eq!(f.v3.position_count(), 0_i128);
    }

    #[test]
    fn test_migrate_past_deadline_aborts_atomically() {
        let env = Env::default();
        let f = fixture(&env);

        env.ledger().set_timestamp(1_000);

        let before_shares = f.v2_lp.balance(&f.lp);
        let before_ta = f.ta.balance(&f.lp);
        let before_tb = f.tb.balance(&f.lp);

        let result = f.migration.try_migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &999_u64,
        );
        assert!(result.is_err(), "expired deadline must abort the migration");

        assert_eq!(f.v2_lp.balance(&f.lp), before_shares);
        assert_eq!(f.ta.balance(&f.lp), before_ta);
        assert_eq!(f.tb.balance(&f.lp), before_tb);
        assert_eq!(f.v3.position_count(), 0_i128);
    }

    #[test]
    fn test_migrate_v2_slippage_aborts_atomically() {
        let env = Env::default();
        let f = fixture(&env);

        let before_shares = f.v2_lp.balance(&f.lp);

        // min_amount_a impossibly high: the real AMM rejects the withdrawal.
        let result = f.migration.try_migrate(
            &f.lp,
            &f.lp_shares,
            &i128::MAX,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );
        assert!(result.is_err(), "impossible V2 slippage must revert");

        assert_eq!(f.v2_lp.balance(&f.lp), before_shares);
        assert_eq!(f.v3.position_count(), 0_i128);
        assert_eq!(f.ta.balance(&f.migration_addr), 0_i128);
        assert_eq!(f.tb.balance(&f.migration_addr), 0_i128);
    }

    /// The atomicity guarantee under the issue's exact scenario: the V2 side
    /// fully succeeds (shares burned and tokens withdrawn by the real AMM), but
    /// the V3 deposit later fails on `min_deposit_a` — the whole migration must
    /// roll back to exactly the pre-call state, including re-minting the burned
    /// V2 LP shares and restoring every token balance.
    #[test]
    fn test_migrate_v3_slippage_aborts_entire_migration_atomically() {
        let env = Env::default();
        let f = fixture(&env);

        let before_shares = f.v2_lp.balance(&f.lp);
        let before_ta = f.ta.balance(&f.lp);
        let before_tb = f.tb.balance(&f.lp);
        let before_ta_contract = f.ta.balance(&f.migration_addr);
        let before_tb_contract = f.tb.balance(&f.migration_addr);

        let v2 = AmmPoolClient::new(&env, &f.v2_pool);
        let before_info = v2.get_info();

        let result = f.migration.try_migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &i128::MAX, // impossible: no position can satisfy this floor
            &0_i128,
            &DEADLINE,
        );
        assert!(
            result.is_err(),
            "a V3 deposit below min_deposit_a must abort the migration"
        );

        // Zero state changes: the burned shares were re-minted by the rollback.
        assert_eq!(f.v2_lp.balance(&f.lp), before_shares);
        assert_eq!(f.ta.balance(&f.lp), before_ta);
        assert_eq!(f.tb.balance(&f.lp), before_tb);
        assert_eq!(f.ta.balance(&f.migration_addr), before_ta_contract);
        assert_eq!(f.tb.balance(&f.migration_addr), before_tb_contract);

        let after_info = v2.get_info();
        assert_eq!(after_info.reserve_a, before_info.reserve_a);
        assert_eq!(after_info.reserve_b, before_info.reserve_b);
        assert_eq!(after_info.total_shares, before_info.total_shares);

        // The V3 pool never saw the funds: no position, no token balance.
        assert_eq!(f.v3.position_count(), 0_i128);
        assert_eq!(f.ta.balance(&f.v3_pool), 0_i128);
        assert_eq!(f.tb.balance(&f.v3_pool), 0_i128);

        // And a follow-up migration still succeeds from the untouched state.
        let retry = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );
        assert_eq!(f.v2_lp.balance(&f.lp), 0_i128);
        assert_eq!(f.v3.position_count(), 1_i128);
        assert_eq!(retry.deposited_a + retry.deposited_b, 2_000_000_i128);
    }

    #[test]
    fn test_migrate_with_wrong_address_auth_is_rejected() {
        let env = Env::default();
        env.budget().reset_unlimited();

        let admin = Address::generate(&env);
        let provider = Address::generate(&env);
        let attacker = Address::generate(&env);
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);

        let v2_mock = env.register_contract(None, MockV2Pool);
        MockV2PoolClient::new(&env, &v2_mock).set_v2_tokens(&ta, &tb);
        let v3_mock = env.register_contract(None, RangePool);
        RangePoolClient::new(&env, &v3_mock).set_tokens(&ta, &tb);

        let contract_addr = env.register_contract(None, MigrationContract);
        let client = MigrationContractClient::new(&env, &contract_addr);

        // Real auth checking: only admin, and only for initialize.
        env.mock_auths(&[MockAuth {
            address: &admin,
            invoke: &MockAuthInvoke {
                contract: &contract_addr,
                fn_name: "initialize",
                args: (admin.clone(), v2_mock.clone(), v3_mock.clone()).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert_eq!(
            client.try_initialize(&admin, &v2_mock, &v3_mock),
            Ok(Ok(())),
            "initialize with an authorized admin must succeed"
        );

        // Only the *wrong* address is authorized for the migration call: the LP
        // (provider) is not, so `provider.require_auth()` fails immediately.
        env.mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: &contract_addr,
                fn_name: "migrate",
                args: (
                    provider.clone(),
                    1_i128,
                    0_i128,
                    0_i128,
                    100_i32,
                    200_i32,
                    0_i32,
                    0_i128,
                    0_i128,
                    DEADLINE,
                )
                    .into_val(&env),
                sub_invokes: &[],
            },
        }]);
        let result = client.try_migrate(
            &provider, &1_i128, &0_i128, &0_i128, &100_i32, &200_i32, &0_i32, &0_i128, &0_i128,
            &DEADLINE,
        );
        assert!(
            result.is_err(),
            "migration without the provider's authorization must fail"
        );

        // The contract was not corrupted: it still serves read-only previews.
        assert_eq!(client.preview_range(&100, &200, &0), (100, 200));
    }

    /// Reversed V2/V3 token pairs are mapped into the CL pool's token order.
    #[test]
    fn test_migrate_reversed_v3_token_order_maps_amounts_correctly() {
        let env = Env::default();
        // Asymmetric deposit so amount_a != amount_b, making any swap bug
        // observable instead of silently self-cancelling:
        //   admin: 6M/6M → reserves 6M/6M, total 6M shares (5_999_000 + 1_000);
        //   lp:    1M/2M → shares = min(1M*6M/6M, 2M*6M/6M) = 1M;
        //   reserves 7M/8M, total shares 7M.
        // Full exit: out_a = 1M*7M/7M = 1_000_000, out_b = 1M*8M/7M = 1_142_857.
        let f = build_fixture(&env, 1_000_000, 2_000_000, true, 0);

        let result = f.migration.migrate(
            &f.lp,
            &f.lp_shares,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &500_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // The reversed V3 pool (token_a == V2's token_b) must receive the tb
        // side as its amount_a and the ta side as its amount_b.
        assert_eq!(result.deposited_a, 1_142_857_i128);
        assert_eq!(result.deposited_b, 1_000_000_i128);

        let pos = f.v3.position(&result.position_token_id.unwrap());
        assert_eq!(pos.deposited_a, 1_142_857_i128);
        assert_eq!(pos.deposited_b, 1_000_000_i128);

        // The underlying tokens landed on the correct sides: V3's token_a (==
        // V2's token_b) is ahead by 1_142_857; V3's token_b (== V2's token_a)
        // by 1_000_000.
        assert_eq!(f.tb.balance(&f.v3_pool), 1_142_857_i128);
        assert_eq!(f.ta.balance(&f.v3_pool), 1_000_000_i128);
    }

    #[test]
    fn test_migrated_event_emits_versioned_schema() {
        let env = Env::default();
        env.mock_all_auths();
        let f = build_fixture(&env, 1_000_000_i128, 1_000_000_i128, false, 1000);

        let _result = f.migration.migrate(
            &f.lp,
            &500_000_i128,
            &0_i128,
            &0_i128,
            &i32::MIN,
            &i32::MAX,
            &50_i32,
            &0_i128,
            &0_i128,
            &DEADLINE,
        );

        // Read all events and find the migrated event
        let events = env.events().all();
        let migrated_events: std::vec::Vec<_> = events
            .iter()
            .filter(|event| {
                if let Some(topic_val) = event.1.get(0) {
                    Symbol::try_from_val(&env, &topic_val) == Ok(Symbol::new(&env, "migrated"))
                } else {
                    false
                }
            })
            .collect();

        assert!(
            !migrated_events.is_empty(),
            "migrated event must be emitted"
        );

        let event = migrated_events.last().unwrap();
        type MigratedEvent = (u32, (i128, i128, i128, Option<u64>, i128, i128));
        let (
            version,
            (_v2_shares, _deposited_a, _deposited_b, _position_token_id, _leftover_a, _leftover_b),
        ): MigratedEvent = MigratedEvent::try_from_val(&env, &event.2)
            .expect("must decode event with version prefix");
        assert_eq!(
            version,
            soroban_amm_sdk::EVENT_SCHEMA_VERSION,
            "event must have correct schema version prefix"
        );
    }
}
