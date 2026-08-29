//! Concentrated Liquidity pool simulator.
//!
//! This module provides a full implementation of the on-chain Soroban CL pool,
//! enabling off-chain simulation, backtesting, and LP outcome analysis.

pub mod analytics;
pub mod math;
pub mod pool;

pub use pool::{ClPoolState, Position, Tick};
