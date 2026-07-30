//! V2 → V3 AMM Migration Contract (Issue #266)
//!
//! Atomically migrates a liquidity position from a V2 constant-product pool
//! to a V3 concentrated-liquidity pool in a single transaction.
//!
//! Flow:
//!   1. LP approves this contract to act on their behalf.
//!   2. LP calls `migrate` with their V2 LP share amount and desired V3 range.
//!   3. Contract burns V2 shares → receives token_a + token_b.
//!   4. Contract deposits into V3 pool at the computed optimal range.
//!   5. Any leftover tokens (due to range asymmetry) are returned to the LP.
//!   6. A migration-incentive fee discount is applied: the V3 deposit fee is
//!      waived for migrating LPs (enforced via a discount flag on the V3 pool).

#![no_std]

use soroban_sdk::token::Client as TokenClient;
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

/// Minimal V3 concentrated-liquidity interface needed for migration.
#[contractclient(name = "V3PoolClient")]
pub trait V3PoolInterface {
    /// Add liquidity within a price range [tick_lower, tick_upper].
    /// Returns the LP NFT position ID minted to `provider`.
    #[allow(clippy::too_many_arguments)]
    fn add_liquidity_range(
        env: Env,
        provider: Address,
        amount_a: i128,
        amount_b: i128,
        tick_lower: i32,
        tick_upper: i32,
        min_shares: i128,
        deadline: u64,
        fee_discount: bool,
    ) -> Result<i128, soroban_sdk::Error>;

    fn get_current_tick(env: Env) -> i32;

    /// Returns the V3 pool's token pair (token_a, token_b).
    fn get_tokens(env: Env) -> (Address, Address);
}

// ── Migration result ──────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationResult {
    /// V3 position ID (LP NFT) minted to the migrating LP.
    pub position_id: i128,
    /// Amount of token_a deposited into V3.
    pub deposited_a: i128,
    /// Amount of token_b deposited into V3.
    pub deposited_b: i128,
    /// Leftover token_a returned to the LP (range asymmetry dust).
    pub refund_a: i128,
    /// Leftover token_b returned to the LP.
    pub refund_b: i128,
    /// Optimal tick_lower computed for the V3 range.
    pub tick_lower: i32,
    /// Optimal tick_upper computed for the V3 range.
    pub tick_upper: i32,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct MigrationContract;

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
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(MigrationError::AlreadyInitialized);
        }

        admin.require_auth();

        // Verify V2 and V3 pools trade the same token pair.
        let v2_client = V2PoolClient::new(&env, &v2_pool);
        let v2_info = v2_client.get_info();

        let v3_client = V3PoolClient::new(&env, &v3_pool);
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
    /// - `min_v3_shares`  – Minimum V3 position size (slippage guard on deposit).
    /// - `deadline`       – Latest ledger timestamp at which this call is valid.
    ///
    /// # Returns
    /// A [`MigrationResult`] describing what was deposited, the V3 position ID,
    /// and any dust refunded to the LP.
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
        min_v3_shares: i128,
        deadline: u64,
    ) -> Result<MigrationResult, MigrationError> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(MigrationError::NotInitialized);
        }

        if v2_shares <= 0 {
            return Err(MigrationError::ZeroShares);
        }

        provider.require_auth();

        let v2_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::V2Pool)
            .unwrap();

        let v3_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::V3Pool)
            .unwrap();

        // ── Step 0: verify V2 and V3 pools trade the same pair ──────────────
        let v2_client = V2PoolClient::new(&env, &v2_pool);
        let pool_info = v2_client.get_info();

        let token_a = pool_info.token_a.clone();
        let token_b = pool_info.token_b.clone();

        let v3_client = V3PoolClient::new(&env, &v3_pool);
        let (v3_token_a, v3_token_b) = v3_client.get_tokens();

        if !((token_a == v3_token_a && token_b == v3_token_b)
            || (token_a == v3_token_b && token_b == v3_token_a))
        {
            return Err(MigrationError::TokenMismatch);
        }

        // ── Step 1: withdraw from V2 ─────────────────────────────────────────
        let (received_a, received_b) =
            v2_client.remove_liquidity(&provider, &v2_shares, &min_a, &min_b, &deadline);

        // ── Step 2: compute optimal V3 tick range ────────────────────────────
        let (final_tick_lower, final_tick_upper) = Self::compute_range(
            &env,
            &v3_client,
            tick_lower,
            tick_upper,
            range_width_ticks,
        )?;

        // ── Step 3: deposit into V3 with fee discount for migrating LPs ─────
        // Provider transfers tokens to this contract so we can forward them.
        let ta_client = TokenClient::new(&env, &token_a);
        let tb_client = TokenClient::new(&env, &token_b);
        let contract_addr = env.current_contract_address();

        // Snapshot balances before this migration's funds land, so the refund
        // step below only ever returns the delta attributable to this call —
        // never any balance already sitting at this shared contract address.
        let balance_a_before = ta_client.balance(&contract_addr);
        let balance_b_before = tb_client.balance(&contract_addr);

        ta_client.transfer(&provider, &contract_addr, &received_a);
        tb_client.transfer(&provider, &contract_addr, &received_b);

        // Approve V3 pool to pull from this contract.
        // live_until_ledger must be >= current ledger sequence when amount > 0.
        // Adding a small lookahead is sufficient because the approval is consumed
        // in the very next call (add_liquidity_range) within the same transaction.
        let approve_expiry = env.ledger().sequence() + 100;

        ta_client.approve(
            &contract_addr,
            &v3_pool,
            &received_a,
            &approve_expiry,
        );

        tb_client.approve(
            &contract_addr,
            &v3_pool,
            &received_b,
            &approve_expiry,
        );

        let position_id = v3_client.add_liquidity_range(
            &contract_addr,
            &received_a,
            &received_b,
            &final_tick_lower,
            &final_tick_upper,
            &min_v3_shares,
            &deadline,
            &true, // fee_discount: migration incentive
        );

        // ── Revoke approvals granted to v3_pool (fix #542) ───────────────────
        // Setting amount=0 with any expiry revokes the allowance. A ledger of 0
        // is only valid when the amount is 0 (SEP-41 permits it), so we use the
        // current ledger sequence which is always valid.
        let revoke_expiry = env.ledger().sequence();

        ta_client.approve(
            &contract_addr,
            &v3_pool,
            &0,
            &revoke_expiry,
        );

        tb_client.approve(
            &contract_addr,
            &v3_pool,
            &0,
            &revoke_expiry,
        );

        // ── Step 4: refund leftover dust to provider ──────────────────────────
        // Computed as the call-scoped delta, not the contract's absolute
        // balance, so pre-existing tokens at this shared address are never
        // swept up and misattributed to this migration.
        let refund_a = ta_client.balance(&contract_addr) - balance_a_before;
        let refund_b = tb_client.balance(&contract_addr) - balance_b_before;

        if refund_a > 0 {
            ta_client.transfer(&contract_addr, &provider, &refund_a);
        }

        if refund_b > 0 {
            tb_client.transfer(&contract_addr, &provider, &refund_b);
        }

        let deposited_a = received_a - refund_a;
        let deposited_b = received_b - refund_b;

        env.events().publish(
            (soroban_sdk::Symbol::new(&env, "migrated"), provider.clone()),
            (
                v2_shares,
                deposited_a,
                deposited_b,
                position_id,
                refund_a,
                refund_b,
            ),
        );

        Ok(MigrationResult {
            position_id,
            deposited_a,
            deposited_b,
            refund_a,
            refund_b,
            tick_lower: final_tick_lower,
            tick_upper: final_tick_upper,
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

        let v3_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::V3Pool)
            .unwrap();

        let v3_client = V3PoolClient::new(&env, &v3_pool);

        Self::compute_range(
            &env,
            &v3_client,
            tick_lower,
            tick_upper,
            range_width_ticks,
        )
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
        env: &Env,
        v3_client: &V3PoolClient,
        tick_lower: i32,
        tick_upper: i32,
        range_width_ticks: i32,
    ) -> Result<(i32, i32), MigrationError> {
        let lower_auto = tick_lower == i32::MIN;
        let upper_auto = tick_upper == i32::MAX;

        let _ = env; // suppress unused warning

        // Both explicit: keep the caller-provided values exactly as-is.
        if !lower_auto && !upper_auto {
            if tick_lower >= tick_upper {
                return Err(MigrationError::InvalidRange);
            }

            return Ok((tick_lower, tick_upper));
        }

        // At least one side is auto-computed, so a positive width is required.
        if range_width_ticks <= 0 {
            return Err(MigrationError::InvalidRange);
        }

        let current_tick = v3_client.get_current_tick();

        // Only compute the bounds that were explicitly marked with sentinels.
        // An explicit lower/upper bound must never be silently overwritten.
        let lower = if lower_auto {
            current_tick
                .checked_sub(range_width_ticks)
                .ok_or(MigrationError::InvalidRange)?
        } else {
            tick_lower
        };

        let upper = if upper_auto {
            current_tick
                .checked_add(range_width_ticks)
                .ok_or(MigrationError::InvalidRange)?
        } else {
            tick_upper
        };

        if lower >= upper {
            return Err(MigrationError::InvalidRange);
        }

        Ok((lower, upper))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    /// Minimal V3 pool stub: only `get_current_tick` is needed to exercise
    /// `compute_range` via the public `preview_range` entry point.
    #[contract]
    struct MockV3Pool;

    #[contractimpl]
    impl MockV3Pool {
        pub fn get_current_tick(_env: Env) -> i32 {
            1_000
        }
    }

    fn setup() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let v2_pool = Address::generate(&env); // unused by preview_range
        let v3_pool = env.register_contract(None, MockV3Pool);

        let contract_addr = env.register_contract(None, MigrationContract);

        MigrationContractClient::new(&env, &contract_addr)
            .initialize(&admin, &v2_pool, &v3_pool);

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

        assert_eq!(
            result,
            Err(Ok(MigrationError::InvalidRange))
        );
    }

    #[test]
    fn partial_auto_range_rejects_inverted_result() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        // Explicit lower (2000) ends up above the auto-computed upper
        // (current_tick + 50 = 1050); this must be rejected, not silently
        // deposited into an inverted/degenerate range.
        let result = client.try_preview_range(&2_000, &i32::MAX, &50);

        assert_eq!(
            result,
            Err(Ok(MigrationError::InvalidRange))
        );
    }

    #[test]
    fn explicit_ticks_reject_inverted_range() {
        let (env, contract_addr) = setup();

        let client = MigrationContractClient::new(&env, &contract_addr);

        let result = client.try_preview_range(&200, &100, &0);

        assert_eq!(
            result,
            Err(Ok(MigrationError::InvalidRange))
        );
    }
}
