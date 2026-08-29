//! Off-chain Soroban AMM simulation engine.
//!
//! The simulator mirrors the on-chain constant-product math used by
//! `contracts/amm` so strategy testing, historical replay, and Monte Carlo
//! experiments can use the same pricing model without paying gas.
//!
//! Now includes concentrated liquidity (CL) pool support for V3-style simulation,
//! with full tick math and LP outcome analytics.

pub mod cl;

mod cli;
mod engine;
mod error;
mod io;
mod monte_carlo;
pub mod pool;
mod replay;

pub use cli::{run, Cli};
pub use engine::{AmmSimulator, SimulationStep};
pub use error::SimulationError;
pub use monte_carlo::{MonteCarloConfig, MonteCarloReport};
pub use pool::{LiquidityQuote, PoolState, SwapQuote, SwapResult};
pub use replay::{ReplayReport, ReplaySummary, TradeAction, TradeOutcome, TradeRecord};

pub use io::{load_pool_state, load_trade_records, save_json_pretty};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_quote_derives_from_fee_formula() {
        let pool = PoolState::new("A", "B", 30).unwrap();
        let pool = PoolState {
            reserve_a: 1_000_000,
            reserve_b: 1_000_000,
            total_shares: 1_000_000,
            ..pool
        };

        let amount_in = 100_000_i128;
        let amount_out = pool.quote_swap_exact_in("A", amount_in).unwrap();

        // The output must equal the constant-product formula with the fee applied,
        // not a frozen literal.
        let fee_bps = 30_i128;
        let with_fee = amount_in * (10_000 - fee_bps);
        let expected_out = with_fee * pool.reserve_b / (pool.reserve_a * 10_000 + with_fee);
        assert_eq!(amount_out.amount_out, expected_out);

        // The fee is the truncated basis-point share of the input, not a literal.
        let expected_fee = amount_in * fee_bps / 10_000;
        assert_eq!(amount_out.fee_amount, expected_fee);
        assert!(amount_out.price_impact_bps >= 0);
    }

    #[test]
    fn replay_tracks_success_and_failure() {
        let pool = PoolState {
            token_a: "A".into(),
            token_b: "B".into(),
            reserve_a: 1_000_000,
            reserve_b: 1_000_000,
            total_shares: 1_000_000,
            fee_bps: 30,
            protocol_fee_bps: 0,
            accrued_fee_a: 0,
            accrued_fee_b: 0,
            price_cumulative_a: 0,
            price_cumulative_b: 0,
            last_timestamp: 0,
            paused: false,
        };
        // The second trade is an exact-out swap whose required input is derived
        // from the same formula as the first trade's quote, so the hardcoded
        // `max_in = 1` is provably too small → a genuine failure (not a literal).
        let required_in = pool.quote_swap_exact_out("B", 50_000).unwrap().amount_in;
        assert!(
            required_in > 1,
            "exact-out input must exceed the tiny max_in"
        );

        let trades = vec![
            TradeRecord {
                timestamp: 1,
                label: None,
                action: TradeAction::SwapExactIn {
                    token_in: "A".into(),
                    amount_in: 100_000,
                    min_out: 0,
                },
            },
            TradeRecord {
                timestamp: 2,
                label: None,
                action: TradeAction::SwapExactOut {
                    token_out: "B".into(),
                    amount_out: 50_000,
                    max_in: Some(1),
                },
            },
        ];

        let mut simulator = AmmSimulator::new(pool);
        let report = simulator.replay(&trades).unwrap();
        assert_eq!(report.summary.trades, 2);
        assert_eq!(report.summary.successful_trades, 1);
        assert_eq!(report.summary.failed_trades, 1);
        assert_eq!(report.steps.len(), 2);
    }
}

// force rebuild