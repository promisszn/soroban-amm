use crate::cl::ClPoolState;
use crate::engine::AmmSimulator;
use crate::error::{Result, SimulationError};
use crate::io::{load_pool_state, load_trade_records};
use crate::monte_carlo::{MonteCarloConfig, MonteCarloReport};
use clap::{Args, Parser, Subcommand};
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "amm-sim", about = "Off-chain Soroban AMM simulator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Quote a swap without mutating pool state.
    Quote(QuoteArgs),
    /// Replay a historical trade log.
    Replay(ReplayArgs),
    /// Run a Monte Carlo stress test against a trade log.
    MonteCarlo(MonteCarloArgs),
    /// Quote a concentrated-liquidity swap from a ClPoolState JSON file.
    ClQuote(ClQuoteArgs),
    /// Replay concentrated-liquidity swaps from JSON.
    ClReplay(ClReplayArgs),
}

#[derive(Args, Debug)]
pub struct QuoteArgs {
    /// Pool state JSON file.
    #[arg(long)]
    pub pool: PathBuf,
    /// Token being sold.
    #[arg(long)]
    pub token_in: Option<String>,
    /// Exact input amount.
    #[arg(long)]
    pub amount_in: Option<i128>,
    /// Token being bought.
    #[arg(long)]
    pub token_out: Option<String>,
    /// Exact output amount.
    #[arg(long)]
    pub amount_out: Option<i128>,
    /// Pretty-print JSON output.
    #[arg(long, default_value_t = false)]
    pub pretty: bool,
    #[arg(long, conflicts_with = "pretty")]
    pub compact: bool,
}

#[derive(Args, Debug)]
pub struct ReplayArgs {
    /// Pool state JSON file.
    #[arg(long)]
    pub pool: PathBuf,
    /// JSON or CSV trade log.
    #[arg(long)]
    pub trades: PathBuf,
    /// Emit indented JSON (compact JSON is the default).
    #[arg(long)]
    pub pretty: bool,
    #[arg(long, conflicts_with = "pretty")]
    pub compact: bool,
}

#[derive(Args, Debug)]
pub struct MonteCarloArgs {
    /// Pool state JSON file.
    #[arg(long)]
    pub pool: PathBuf,
    /// JSON or CSV trade log.
    #[arg(long)]
    pub trades: PathBuf,
    /// Number of Monte Carlo runs.
    #[arg(long, default_value_t = 1000)]
    pub iterations: usize,
    /// Amount perturbation in basis points.
    #[arg(long, default_value_t = 0)]
    pub amount_shock_bps: u32,
    /// Shuffle trade order before each run.
    #[arg(long, default_value_t = false)]
    pub shuffle_trades: bool,
    /// RNG seed for reproducible runs.
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// Emit indented JSON (compact JSON is the default).
    #[arg(long)]
    pub pretty: bool,
    #[arg(long, conflicts_with = "pretty")]
    pub compact: bool,
}

#[derive(Args, Debug)]
pub struct ClQuoteArgs {
    /// JSON file containing a serialized ClPoolState.
    #[arg(long)]
    pub pool: PathBuf,
    /// Direction: token A for token B.
    #[arg(long)]
    pub zero_for_one: bool,
    #[arg(long)]
    pub amount_in: i128,
    /// Sqrt price Q64.96 limit; zero means no limit.
    #[arg(long, default_value_t = 0)]
    pub sqrt_price_limit_x96: i128,
    #[arg(long, default_value_t = 0)]
    pub min_out: i128,
    #[arg(long)]
    pub pretty: bool,
}

#[derive(Args, Debug)]
pub struct ClReplayArgs {
    #[arg(long)]
    pub pool: PathBuf,
    /// JSON array of {zero_for_one, amount_in, sqrt_price_limit_x96, min_out}.
    #[arg(long)]
    pub swaps: PathBuf,
    #[arg(long)]
    pub pretty: bool,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Quote(args) => quote(args),
        Commands::Replay(args) => replay(args),
        Commands::MonteCarlo(args) => monte_carlo(args),
        Commands::ClQuote(args) => cl_quote(args),
        Commands::ClReplay(args) => cl_replay(args),
    }
}

fn quote(args: QuoteArgs) -> Result<()> {
    let QuoteArgs {
        pool,
        token_in,
        amount_in,
        token_out,
        amount_out,
        pretty,
        compact: _,
    } = args;
    let pool = load_pool_state(&pool)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    if let (Some(token_in), Some(amount_in)) = (token_in, amount_in) {
        let quote = pool.quote_swap_exact_in(&token_in, amount_in)?;
        if pretty {
            write_json(&mut handle, &quote, true)?;
        } else {
            write_json(&mut handle, &quote, false)?;
        }
    } else if let (Some(token_out), Some(amount_out)) = (token_out, amount_out) {
        let quote = pool.quote_swap_exact_out(&token_out, amount_out)?;
        if pretty {
            write_json(&mut handle, &quote, true)?;
        } else {
            write_json(&mut handle, &quote, false)?;
        }
    } else {
        return Err(SimulationError::InvalidInput(
            "provide either --token-in + --amount-in or --token-out + --amount-out".into(),
        ));
    }

    Ok(())
}

fn replay(args: ReplayArgs) -> Result<()> {
    let pool = load_pool_state(&args.pool)?;
    let trades = load_trade_records(&args.trades)?;
    let mut simulator = AmmSimulator::new(pool);
    let report = simulator.replay(&trades)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if args.pretty {
        write_json(&mut handle, &report, true)?;
    } else {
        write_json(&mut handle, &report, false)?;
    }
    if report.summary.failed_trades > 0 {
        Err(SimulationError::FailedTrades(report.summary.failed_trades))
    } else {
        Ok(())
    }
}

fn monte_carlo(args: MonteCarloArgs) -> Result<()> {
    let pool = load_pool_state(&args.pool)?;
    let trades = load_trade_records(&args.trades)?;
    let report = MonteCarloReport::run(
        &pool,
        &trades,
        MonteCarloConfig {
            iterations: args.iterations,
            amount_shock_bps: args.amount_shock_bps,
            shuffle_trades: args.shuffle_trades,
            seed: args.seed,
        },
    )?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if args.pretty {
        write_json(&mut handle, &report, true)?;
    } else {
        write_json(&mut handle, &report, false)?;
    }
    Ok(())
}

fn write_json<T: serde::Serialize>(out: &mut impl Write, value: &T, pretty: bool) -> Result<()> {
    let bytes = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    }
    .map_err(|e| SimulationError::InvalidInput(e.to_string()))?;
    match out.write_all(&bytes).and_then(|_| out.write_all(b"\n")) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(source) => Err(SimulationError::Io {
            path: "stdout".into(),
            source,
        }),
        Ok(()) => Ok(()),
    }
}

fn cl_quote(args: ClQuoteArgs) -> Result<()> {
    #[derive(serde::Serialize)]
    struct Output<'a> {
        result: &'a crate::cl::ClSwapResult,
        pool: &'a ClPoolState,
    }
    let text = std::fs::read_to_string(&args.pool).map_err(|source| SimulationError::Io {
        path: args.pool.display().to_string(),
        source,
    })?;
    let mut pool: ClPoolState =
        serde_json::from_str(&text).map_err(|source| SimulationError::Json {
            path: args.pool.display().to_string(),
            source,
        })?;
    let result = pool.swap(
        args.zero_for_one,
        args.amount_in,
        args.sqrt_price_limit_x96,
        args.min_out,
    )?;
    let mut out = io::stdout().lock();
    write_json(
        &mut out,
        &Output {
            result: &result,
            pool: &pool,
        },
        args.pretty,
    )
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ClReplaySwap {
    zero_for_one: bool,
    amount_in: i128,
    #[serde(default)]
    sqrt_price_limit_x96: i128,
    #[serde(default)]
    min_out: i128,
}

#[derive(Debug, serde::Serialize)]
struct ClReplayStep {
    swap: ClReplaySwap,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<crate::cl::ClSwapResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn cl_replay(args: ClReplayArgs) -> Result<()> {
    let p = args.pool;
    let text = std::fs::read_to_string(&p).map_err(|source| SimulationError::Io {
        path: p.display().to_string(),
        source,
    })?;
    let mut pool: ClPoolState =
        serde_json::from_str(&text).map_err(|source| SimulationError::Json {
            path: p.display().to_string(),
            source,
        })?;
    let p = args.swaps;
    let text = std::fs::read_to_string(&p).map_err(|source| SimulationError::Io {
        path: p.display().to_string(),
        source,
    })?;
    let swaps: Vec<ClReplaySwap> =
        serde_json::from_str(&text).map_err(|source| SimulationError::Json {
            path: p.display().to_string(),
            source,
        })?;
    run_cl_replay(&mut pool, swaps, &mut io::stdout().lock(), args.pretty)
}

fn run_cl_replay(
    pool: &mut ClPoolState,
    swaps: Vec<ClReplaySwap>,
    out: &mut impl Write,
    pretty: bool,
) -> Result<()> {
    #[derive(serde::Serialize)]
    struct Output<'a> {
        results: &'a [ClReplayStep],
        failed_swaps: usize,
        pool: &'a ClPoolState,
    }
    let results: Vec<ClReplayStep> = swaps
        .into_iter()
        .map(|swap| {
            match pool.swap(
                swap.zero_for_one,
                swap.amount_in,
                swap.sqrt_price_limit_x96,
                swap.min_out,
            ) {
                Ok(result) => ClReplayStep {
                    swap,
                    result: Some(result),
                    error: None,
                },
                Err(err) => ClReplayStep {
                    swap,
                    result: None,
                    error: Some(err.to_string()),
                },
            }
        })
        .collect();
    let failed_swaps = results.iter().filter(|step| step.error.is_some()).count();
    write_json(
        out,
        &Output {
            results: &results,
            failed_swaps,
            pool,
        },
        pretty,
    )?;
    if failed_swaps > 0 {
        Err(SimulationError::FailedSwaps(failed_swaps))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cl::ClSwapResult;

    #[derive(serde::Deserialize)]
    struct Step {
        swap: ClReplaySwap,
        result: Option<ClSwapResult>,
        error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct Report {
        results: Vec<Step>,
        failed_swaps: usize,
        pool: ClPoolState,
    }

    fn pool() -> ClPoolState {
        let mut pool = ClPoolState::new("XLM", "USDC", 30, 10).unwrap();
        pool.initialize_at_tick(0).unwrap();
        pool.add_liquidity("lp", -600, 600, 10_000_000_000_000)
            .unwrap();
        pool
    }

    fn swap(amount_in: i128, min_out: i128) -> ClReplaySwap {
        ClReplaySwap {
            zero_for_one: true,
            amount_in,
            sqrt_price_limit_x96: 0,
            min_out,
        }
    }

    fn replay(swaps: Vec<ClReplaySwap>) -> (Result<()>, Report, ClPoolState) {
        let mut pool = pool();
        let mut out = Vec::new();
        let result = run_cl_replay(&mut pool, swaps, &mut out, false);
        (result, serde_json::from_slice(&out).unwrap(), pool)
    }

    #[test]
    fn all_successful_swaps_report_results_and_succeed() {
        let mut expected_pool = pool();
        let expected = [
            expected_pool.swap(true, 1_000_000, 0, 0).unwrap(),
            expected_pool.swap(true, 2_000_000, 0, 0).unwrap(),
        ];

        let (result, report, final_pool) = replay(vec![swap(1_000_000, 0), swap(2_000_000, 0)]);

        assert!(result.is_ok());
        assert_eq!(report.failed_swaps, 0);
        assert_eq!(report.results.len(), 2);
        for (step, expected) in report.results.iter().zip(&expected) {
            assert_eq!(step.result.as_ref(), Some(expected));
            assert_eq!(step.error, None);
        }
        assert_eq!(final_pool, expected_pool);
        assert_eq!(report.pool, expected_pool);
    }

    #[test]
    fn mid_file_failure_keeps_earlier_and_later_results() {
        let mut expected_pool = pool();
        let first = expected_pool.swap(true, 1_000_000, 0, 0).unwrap();
        let last = expected_pool.swap(true, 3_000_000, 0, 0).unwrap();

        let (result, report, final_pool) = replay(vec![
            swap(1_000_000, 0),
            swap(2_000_000, i128::MAX),
            swap(3_000_000, 0),
        ]);

        assert!(matches!(result, Err(SimulationError::FailedSwaps(1))));
        assert_eq!(report.failed_swaps, 1);
        assert_eq!(report.results.len(), 3);
        assert_eq!(report.results[0].result, Some(first));
        assert_eq!(report.results[1].result, None);
        assert_eq!(
            report.results[1].error.as_deref(),
            Some("slippage guard failed")
        );
        assert_eq!(report.results[1].swap.amount_in, 2_000_000);
        assert_eq!(report.results[2].result, Some(last));
        assert_eq!(final_pool, expected_pool);
        assert_eq!(report.pool, expected_pool);
    }

    #[test]
    fn all_failed_swaps_still_print_every_step() {
        let (result, report, final_pool) = replay(vec![swap(0, 0), swap(1_000_000, i128::MAX)]);

        assert!(matches!(result, Err(SimulationError::FailedSwaps(2))));
        assert_eq!(report.failed_swaps, 2);
        let errors: Vec<_> = report.results.iter().map(|s| s.error.as_deref()).collect();
        assert_eq!(
            errors,
            [
                Some("amount must be positive"),
                Some("slippage guard failed")
            ]
        );
        assert!(report.results.iter().all(|s| s.result.is_none()));
        assert_eq!(final_pool, pool());
    }
}
