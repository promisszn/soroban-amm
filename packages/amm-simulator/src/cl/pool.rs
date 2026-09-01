//! Concentrated Liquidity pool model.
//!
//! Mirrors the on-chain contracts/concentrated_liquidity contract.
//! Implements the state and operations needed to simulate CL pool behavior.

use crate::error::{Result, SimulationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tick structure holding liquidity and fee growth data.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Tick {
    /// Net liquidity added at this tick
    pub liquidity_net: i128,
    /// Fee growth accumulated above this tick
    pub fee_growth_outside_a: i128,
    /// Fee growth accumulated above this tick
    pub fee_growth_outside_b: i128,
}

/// Position held by an LP in a concentrated liquidity pool.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Position {
    /// Owner address
    pub owner: String,
    /// Lower tick (inclusive)
    pub lower_tick: i32,
    /// Upper tick (exclusive)
    pub upper_tick: i32,
    /// Liquidity in this position
    pub liquidity: i128,
    /// Fee growth inside snapshot at time of creation
    pub fee_growth_inside_a_snapshot: i128,
    pub fee_growth_inside_b_snapshot: i128,
}

/// State of a concentrated liquidity pool.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClPoolState {
    /// Tokens in the pool
    pub token_a: String,
    pub token_b: String,

    /// Current price in Q64.96 fixed-point format
    pub sqrt_price_x96: i128,

    /// Current active tick
    pub current_tick: i32,

    /// Total liquidity in the active tick range
    pub liquidity: i128,

    /// Global fee growth accumulated in token A (per unit liquidity)
    pub fee_growth_global_a: i128,
    /// Global fee growth accumulated in token B (per unit liquidity)
    pub fee_growth_global_b: i128,

    /// Tick spacing (e.g., 1 tick per step = tick_spacing of 1)
    pub tick_spacing: i32,

    /// Fee in basis points (e.g., 30 = 0.3%)
    pub fee_bps: i128,

    /// Swap fee pool (accumulated but not distributed)
    pub accrued_fee_a: i128,
    pub accrued_fee_b: i128,

    /// Protocol fee pool
    pub protocol_fee_a: i128,
    pub protocol_fee_b: i128,

    /// Map of initialized ticks to their state
    pub ticks: BTreeMap<i32, Tick>,

    /// Positions by ID (owner + lower + upper tick uniquely identify)
    pub positions: BTreeMap<String, Position>,

    /// Last timestamp for TWAP calculation
    pub last_timestamp: u64,

    /// Paused flag
    pub paused: bool,
}

impl ClPoolState {
    /// Create a new concentrated liquidity pool.
    pub fn new(
        token_a: impl Into<String>,
        token_b: impl Into<String>,
        fee_bps: i128,
        tick_spacing: i32,
    ) -> Result<Self> {
        let pool = Self {
            token_a: token_a.into(),
            token_b: token_b.into(),
            sqrt_price_x96: 1_u128.checked_shl(96).ok_or(SimulationError::PriceOverflow)? as i128,
            current_tick: 0,
            liquidity: 0,
            fee_growth_global_a: 0,
            fee_growth_global_b: 0,
            tick_spacing,
            fee_bps,
            accrued_fee_a: 0,
            accrued_fee_b: 0,
            protocol_fee_a: 0,
            protocol_fee_b: 0,
            ticks: BTreeMap::new(),
            positions: BTreeMap::new(),
            last_timestamp: 0,
            paused: false,
        };
        pool.validate()?;
        Ok(pool)
    }

    /// Validate the pool state.
    pub fn validate(&self) -> Result<()> {
        if self.token_a == self.token_b {
            return Err(SimulationError::InvalidToken {
                token: self.token_a.clone(),
            });
        }
        if !(0..=10_000).contains(&self.fee_bps) {
            return Err(SimulationError::InvalidFeeBps {
                fee_bps: self.fee_bps,
            });
        }
        if self.tick_spacing <= 0 || self.tick_spacing > 32767 {
            return Err(SimulationError::InvalidTickSpacing {
                tick_spacing: self.tick_spacing,
            });
        }
        Ok(())
    }

    /// Check if the pool is empty (no liquidity).
    pub fn is_empty(&self) -> bool {
        self.liquidity == 0
    }

    /// Get the spot price of token B in terms of token A.
    ///
    /// Formula: spot_price = (sqrt_price_x96 / 2^96)^2
    pub fn spot_price_b(&self) -> f64 {
        let price_f = self.sqrt_price_x96 as f64 / ((1_u128 << 96) as f64);
        price_f * price_f
    }

    /// Initialize the pool with a starting price.
    pub fn initialize(&mut self, initial_sqrt_price_x96: i128) -> Result<()> {
        if initial_sqrt_price_x96 <= 0 {
            return Err(SimulationError::InvalidPrice);
        }
        self.sqrt_price_x96 = initial_sqrt_price_x96;
        self.current_tick = 0; // TODO: compute tick from price
        Ok(())
    }

    /// Pause the pool.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume the pool.
    pub fn unpause(&mut self) {
        self.paused = false;
    }

    /// Register a tick in the pool (if it's not already initialized).
    pub fn initialize_tick(&mut self, tick: i32) {
        if !self.ticks.contains_key(&tick) {
            self.ticks.insert(
                tick,
                Tick {
                    liquidity_net: 0,
                    fee_growth_outside_a: 0,
                    fee_growth_outside_b: 0,
                },
            );
        }
    }
}
