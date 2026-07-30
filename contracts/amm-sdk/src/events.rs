//! Strongly-typed decoders for every event emitted by the AMM contracts.
//!
//! The AMM contract emits Soroban events with the following structure:
//!
//! | Event symbol    | Topics                        | Data                              |
//! |-----------------|-------------------------------|-----------------------------------|
//! | `swap`          | `("swap", trader)`            | `(1, (token_in, amt_in, token_out, amt_out, referrer))` |
//! | `add_liquidity` | `("add_liquidity", provider)` | `(1, (amount_a, amount_b, shares))`    |
//! | `rm_liq`        | `("rm_liq",)`                 | `(1, (provider, shares, out_a, out_b))`|
//! | `rm_liq_1s`     | `("rm_liq_1s",)`              | `(1, (provider, shares, token_out, total_out))` |
//! | `flash_loan`    | `("flash_loan", receiver)`    | `(1, (token, amount, fee))`            |
//! | `fee_upd`       | `("fee_upd", admin)`          | `(1, (new_fee_bps,))`                  |
//! | `flash_fee_upd` | `("flash_fee_upd", admin)`    | `(1, (new_fee_bps,))`                  |
//! | `admin_nominated`| `("admin_nominated",)`       | `(1, (current_admin, new_admin))`      |
//! | `admin_changed` | `("admin_changed",)`          | `(1, (new_admin,))`                    |
//! | `upgraded`      | `("upgraded",)`               | `(1, (new_wasm_hash,))`                |
//! | `circuit_break` | `("circuit_break",)`          | `(1, (price_before, price_after, deviation_bps, threshold_bps))` |

use soroban_sdk::{contracttype, Address, BytesN};

// ── Event data types ──────────────────────────────────────────────────────────

/// Emitted when a swap executes.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct SwapEvent {
    pub trader: Address,
    pub token_in: Address,
    pub amount_in: i128,
    pub token_out: Address,
    pub amount_out: i128,
    pub referrer: Option<Address>,
}

/// Emitted when liquidity is added.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct AddLiquidityEvent {
    pub provider: Address,
    pub amount_a: i128,
    pub amount_b: i128,
    pub shares_minted: i128,
}

/// Emitted when liquidity is removed (both tokens).
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveLiquidityEvent {
    pub provider: Address,
    pub shares_burned: i128,
    pub amount_a: i128,
    pub amount_b: i128,
}

/// Emitted when liquidity is removed as a single token.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveLiquidityOneSidedEvent {
    pub provider: Address,
    pub shares_burned: i128,
    pub token_out: Address,
    pub total_out: i128,
}

/// Emitted when a flash loan executes.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct FlashLoanEvent {
    pub receiver: Address,
    pub token: Address,
    pub amount: i128,
    pub fee: i128,
}

/// Emitted when the swap fee is updated.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct FeeUpdatedEvent {
    pub admin: Address,
    pub new_fee_bps: i128,
}

/// Emitted when the flash loan fee is updated.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct FlashFeeUpdatedEvent {
    pub admin: Address,
    pub new_fee_bps: i128,
}

/// Emitted when a new admin is nominated.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct AdminNominatedEvent {
    pub current_admin: Address,
    pub new_admin: Address,
}

/// Emitted when admin transfer is accepted and completed.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct AdminChangedEvent {
    pub new_admin: Address,
}

/// Emitted when the contract WASM is upgraded.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradedEvent {
    pub new_wasm_hash: BytesN<32>,
}

/// Emitted when the circuit breaker auto-pauses the pool due to extreme price
/// movement.
#[contracttype]
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitBreakerEvent {
    /// Spot price before the triggering trade (scaled × 1 000 000).
    pub price_before: i128,
    /// Spot price after the triggering trade (scaled × 1 000 000).
    pub price_after: i128,
    /// Measured deviation in basis points.
    pub deviation_bps: i128,
    /// Configured threshold that was exceeded.
    pub threshold_bps: i128,
}

// ── Event symbol constants ────────────────────────────────────────────────────

/// Symbol strings matching the on-chain event topics.
pub mod symbols {
    pub const SWAP: &str = "swap";
    pub const ADD_LIQUIDITY: &str = "add_liquidity";
    pub const REMOVE_LIQUIDITY: &str = "rm_liq";
    pub const REMOVE_LIQUIDITY_ONE_SIDED: &str = "rm_liq_1s";
    pub const FLASH_LOAN: &str = "flash_loan";
    pub const FEE_UPDATED: &str = "fee_upd";
    pub const FLASH_FEE_UPDATED: &str = "flash_fee_upd";
    pub const ADMIN_NOMINATED: &str = "admin_nominated";
    pub const ADMIN_CHANGED: &str = "admin_changed";
    pub const UPGRADED: &str = "upgraded";
    pub const CIRCUIT_BREAKER: &str = "circuit_break";
}

// ── Decoder helpers ───────────────────────────────────────────────────────────

/// Wraps all possible events that can originate from an AMM pool.
pub enum AmmEvent {
    Swap(SwapEvent),
    AddLiquidity(AddLiquidityEvent),
    RemoveLiquidity(RemoveLiquidityEvent),
    RemoveLiquidityOneSided(RemoveLiquidityOneSidedEvent),
    FlashLoan(FlashLoanEvent),
    FeeUpdated(FeeUpdatedEvent),
    FlashFeeUpdated(FlashFeeUpdatedEvent),
    AdminNominated(AdminNominatedEvent),
    AdminChanged(AdminChangedEvent),
    Upgraded(UpgradedEvent),
    CircuitBreaker(CircuitBreakerEvent),
}

/// Decode a raw Soroban event `data` field given its `topics` array and `data` field.
///
/// Returns `None` if the symbol is not recognised, the schema version is mismatched,
/// or the data cannot be decoded.
///
/// # Usage
/// ```rust,ignore
/// use soroban_sdk::{Env, Vec, Val};
/// use soroban_amm_sdk::events::{decode_amm_event, AmmEvent};
///
/// // `topics` and `data` come from the RPC `getEvents` response.
/// if let Some(event) = decode_amm_event(&env, topics, raw_data_val) {
///     match event {
///         AmmEvent::Swap(e) => println!("swap: {} -> {}", e.amount_in, e.amount_out),
///         _ => {}
///     }
/// }
/// ```
///
/// In practice you would obtain the data from `stellar-sdk-rs` or the
/// Soroban RPC `getEvents` endpoint and pass the decoded XDR values here.
pub fn decode_amm_event(
    env: &soroban_sdk::Env,
    topics: soroban_sdk::Vec<soroban_sdk::Val>,
    data: soroban_sdk::Val,
) -> Option<AmmEvent> {
    use soroban_sdk::{TryFromVal, TryIntoVal, Symbol};

    if topics.is_empty() {
        return None;
    }

    let symbol_val = topics.get(0)?;
    let symbol = Symbol::try_from_val(env, &symbol_val).ok()?;

    let (version, payload_val): (u32, soroban_sdk::Val) = TryFromVal::try_from_val(env, &data).ok()?;
    if version != crate::EVENT_SCHEMA_VERSION {
        return None;
    }

    if symbol == Symbol::new(env, symbols::SWAP) {
        let trader: Address = topics.get(1)?.try_into_val(env).ok()?;
        let (token_in, amount_in, token_out, amount_out, referrer): (Address, i128, Address, i128, Option<Address>) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::Swap(SwapEvent {
            trader,
            token_in,
            amount_in,
            token_out,
            amount_out,
            referrer,
        }))
    } else if symbol == Symbol::new(env, symbols::ADD_LIQUIDITY) {
        let provider: Address = topics.get(1)?.try_into_val(env).ok()?;
        let (amount_a, amount_b, shares_minted): (i128, i128, i128) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::AddLiquidity(AddLiquidityEvent {
            provider,
            amount_a,
            amount_b,
            shares_minted,
        }))
    } else if symbol == Symbol::new(env, symbols::REMOVE_LIQUIDITY) {
        let (provider, shares_burned, amount_a, amount_b): (Address, i128, i128, i128) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::RemoveLiquidity(RemoveLiquidityEvent {
            provider,
            shares_burned,
            amount_a,
            amount_b,
        }))
    } else if symbol == Symbol::new(env, symbols::REMOVE_LIQUIDITY_ONE_SIDED) {
        let (provider, shares_burned, token_out, total_out): (Address, i128, Address, i128) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::RemoveLiquidityOneSided(RemoveLiquidityOneSidedEvent {
            provider,
            shares_burned,
            token_out,
            total_out,
        }))
    } else if symbol == Symbol::new(env, symbols::FLASH_LOAN) {
        let receiver: Address = topics.get(1)?.try_into_val(env).ok()?;
        let (token, amount, fee): (Address, i128, i128) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::FlashLoan(FlashLoanEvent {
            receiver,
            token,
            amount,
            fee,
        }))
    } else if symbol == Symbol::new(env, symbols::FEE_UPDATED) {
        let admin: Address = topics.get(1)?.try_into_val(env).ok()?;
        let (new_fee_bps,): (i128,) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::FeeUpdated(FeeUpdatedEvent { admin, new_fee_bps }))
    } else if symbol == Symbol::new(env, symbols::FLASH_FEE_UPDATED) {
        let admin: Address = topics.get(1)?.try_into_val(env).ok()?;
        let (new_fee_bps,): (i128,) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::FlashFeeUpdated(FlashFeeUpdatedEvent { admin, new_fee_bps }))
    } else if symbol == Symbol::new(env, symbols::ADMIN_NOMINATED) {
        let (current_admin, new_admin): (Address, Address) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::AdminNominated(AdminNominatedEvent {
            current_admin,
            new_admin,
        }))
    } else if symbol == Symbol::new(env, symbols::ADMIN_CHANGED) {
        let (new_admin,): (Address,) = TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::AdminChanged(AdminChangedEvent { new_admin }))
    } else if symbol == Symbol::new(env, symbols::UPGRADED) {
        let (new_wasm_hash,): (BytesN<32>,) = TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::Upgraded(UpgradedEvent { new_wasm_hash }))
    } else if symbol == Symbol::new(env, symbols::CIRCUIT_BREAKER) {
        let (price_before, price_after, deviation_bps, threshold_bps): (i128, i128, i128, i128) =
            TryFromVal::try_from_val(env, &payload_val).ok()?;
        Some(AmmEvent::CircuitBreaker(CircuitBreakerEvent {
            price_before,
            price_after,
            deviation_bps,
            threshold_bps,
        }))
    } else {
        None
    }
}
