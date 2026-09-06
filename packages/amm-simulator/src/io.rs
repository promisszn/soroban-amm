use crate::error::{Result, SimulationError};
use crate::pool::PoolState;
use crate::replay::{FlatTradeRecord, TradeRecord};
use serde::Serialize;
use std::fs;
use std::path::Path;

pub fn load_pool_state(path: impl AsRef<Path>) -> Result<PoolState> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| SimulationError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let pool: PoolState =
        serde_json::from_str(&contents).map_err(|source| SimulationError::Json {
            path: path.display().to_string(),
            source,
        })?;
    pool.validate()?;
    Ok(pool)
}

pub fn load_trade_records(path: impl AsRef<Path>) -> Result<Vec<TradeRecord>> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "csv" => load_trade_records_csv(path),
        _ => load_trade_records_json(path),
    }
}

pub fn save_json_pretty<T: Serialize>(path: impl AsRef<Path>, value: &T) -> Result<()> {
    let path = path.as_ref();
    let json = serde_json::to_string_pretty(value).map_err(|source| SimulationError::Json {
        path: path.display().to_string(),
        source,
    })?;
    fs::write(path, json).map_err(|source| SimulationError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn load_trade_records_json(path: &Path) -> Result<Vec<TradeRecord>> {
    let contents = fs::read_to_string(path).map_err(|source| SimulationError::Io {
        path: path.display().to_string(),
        source,
    })?;

    if let Ok(records) = serde_json::from_str::<Vec<TradeRecord>>(&contents) {
        return Ok(records);
    }

    #[derive(serde::Deserialize)]
    struct Wrapper {
        trades: Vec<TradeRecord>,
    }

    serde_json::from_str::<Wrapper>(&contents)
        .map(|wrapper| wrapper.trades)
        .map_err(|source| SimulationError::Json {
            path: path.display().to_string(),
            source,
        })
}

fn load_trade_records_csv(path: &Path) -> Result<Vec<TradeRecord>> {
    let mut reader = csv::Reader::from_path(path).map_err(|source| SimulationError::Csv {
        path: path.display().to_string(),
        source,
    })?;
    let mut records = Vec::new();

    // A CSV row carries the same field names as the flat JSON record, so both
    // formats share one representation and one `kind` -> `TradeAction` mapping.
    for row in reader.deserialize::<FlatTradeRecord>() {
        let row = row.map_err(|source| SimulationError::Csv {
            path: path.display().to_string(),
            source,
        })?;

        records.push(row.into_record().map_err(|kind| {
            SimulationError::InvalidInput(format!("unknown trade kind `{kind}`"))
        })?);
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::TradeAction;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_file(extension: &str) -> std::path::PathBuf {
        // These tests run as parallel threads of a single process and finish in
        // well under a millisecond, so a wall-clock timestamp is not unique
        // enough: two tests could be handed the same path, then read and delete
        // each other's fixture. A process-wide counter cannot collide, and the
        // pid still separates concurrent `cargo test` invocations.
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "amm-simulator-io-test-{}-{}.{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            extension
        ))
    }

    fn write_temp_file(extension: &str, contents: &str) -> std::path::PathBuf {
        let path = temp_file(extension);
        fs::write(&path, contents).expect("test fixture should be writable");
        path
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_trade_records_csv_parses_swap_exact_in() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             100,swap_exact_in,swap-a,XLM,USDC,5000,,,,,4900,,,,\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        let records = result.expect("CSV should parse successfully");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp, 100);
        assert_eq!(records[0].label.as_deref(), Some("swap-a"));

        match &records[0].action {
            TradeAction::SwapExactIn {
                token_in,
                amount_in,
                min_out,
            } => {
                assert_eq!(token_in, "XLM");
                assert_eq!(*amount_in, 5000);
                assert_eq!(*min_out, 4900);
            }
            other => panic!("expected SwapExactIn, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_csv_parses_swap_exact_out() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             200,swap_exact_out,swap-b,,USDC,,3000,,,,,3200,,,\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        let records = result.expect("CSV should parse successfully");
        assert_eq!(records.len(), 1);

        match &records[0].action {
            TradeAction::SwapExactOut {
                token_out,
                amount_out,
                max_in,
            } => {
                assert_eq!(token_out, "USDC");
                assert_eq!(*amount_out, 3000);
                assert_eq!(*max_in, Some(3200));
            }
            other => panic!("expected SwapExactOut, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_csv_parses_add_liquidity() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             300,add_liquidity,deposit,,,,,10000,20000,,,,9000,,\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        let records = result.expect("CSV should parse successfully");
        assert_eq!(records.len(), 1);

        match &records[0].action {
            TradeAction::AddLiquidity {
                amount_a,
                amount_b,
                min_shares,
            } => {
                assert_eq!(*amount_a, 10000);
                assert_eq!(*amount_b, 20000);
                assert_eq!(*min_shares, 9000);
            }
            other => panic!("expected AddLiquidity, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_csv_parses_remove_liquidity() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             400,remove_liquidity,withdraw,,,,,,,2500,,,,2000,3000\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        let records = result.expect("CSV should parse successfully");
        assert_eq!(records.len(), 1);

        match &records[0].action {
            TradeAction::RemoveLiquidity {
                shares,
                min_a,
                min_b,
            } => {
                assert_eq!(*shares, 2500);
                assert_eq!(*min_a, 2000);
                assert_eq!(*min_b, 3000);
            }
            other => panic!("expected RemoveLiquidity, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_csv_unknown_kind_returns_invalid_input() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             500,unknown_trade,test,XLM,USDC,1000,,,,,,,,,\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        match result {
            Err(SimulationError::InvalidInput(message)) => {
                assert_eq!(message, "unknown trade kind `unknown_trade`");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_csv_blank_optional_columns_use_expected_defaults() {
        let path = write_temp_file(
            "csv",
            "timestamp,kind,label,token_in,token_out,amount_in,amount_out,amount_a,amount_b,shares,min_out,max_in,min_shares,min_a,min_b\n\
             600,swap_exact_in,,,,,,,,,,,,,\n\
             601,swap_exact_out,, ,USDC,,,,,,,,,,\n\
             602,add_liquidity,,,,,,1000,2000,,,,,,\n\
             603,remove_liquidity,,,,,,,,500,,,,,\n",
        );

        let result = load_trade_records_csv(&path);

        cleanup(&path);

        let records = result.expect("CSV should parse successfully");
        assert_eq!(records.len(), 4);

        match &records[0].action {
            TradeAction::SwapExactIn {
                token_in,
                amount_in,
                min_out,
            } => {
                assert_eq!(token_in, "");
                assert_eq!(*amount_in, 0);
                assert_eq!(*min_out, 0);
            }
            other => panic!("expected SwapExactIn, got {other:?}"),
        }

        match &records[1].action {
            TradeAction::SwapExactOut {
                token_out,
                amount_out,
                max_in,
            } => {
                assert_eq!(token_out.trim(), "USDC");
                assert_eq!(*amount_out, 0);
                assert_eq!(*max_in, None);
            }
            other => panic!("expected SwapExactOut, got {other:?}"),
        }

        match &records[2].action {
            TradeAction::AddLiquidity {
                amount_a,
                amount_b,
                min_shares,
            } => {
                assert_eq!(*amount_a, 1000);
                assert_eq!(*amount_b, 2000);
                assert_eq!(*min_shares, 0);
            }
            other => panic!("expected AddLiquidity, got {other:?}"),
        }

        match &records[3].action {
            TradeAction::RemoveLiquidity {
                shares,
                min_a,
                min_b,
            } => {
                assert_eq!(*shares, 500);
                assert_eq!(*min_a, 0);
                assert_eq!(*min_b, 0);
            }
            other => panic!("expected RemoveLiquidity, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_json_parses_bare_array() {
        let path = write_temp_file(
            "json",
            r#"[
                {
                    "timestamp": 1000,
                    "label": "json-array",
                    "kind": "swap_exact_in",
                    "token_in": "XLM",
                    "amount_in": 5000,
                    "min_out": 4500
                }
            ]"#,
        );

        let result = load_trade_records_json(&path);

        cleanup(&path);

        let records = result.expect("bare JSON array should parse successfully");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp, 1000);
        assert_eq!(records[0].label.as_deref(), Some("json-array"));

        match &records[0].action {
            TradeAction::SwapExactIn {
                token_in,
                amount_in,
                min_out,
            } => {
                assert_eq!(token_in, "XLM");
                assert_eq!(*amount_in, 5000);
                assert_eq!(*min_out, 4500);
            }
            other => panic!("expected SwapExactIn, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_json_parses_trades_wrapper() {
        let path = write_temp_file(
            "json",
            r#"{
                "trades": [
                    {
                        "timestamp": 2000,
                        "label": "json-wrapper",
                        "kind": "remove_liquidity",
                        "shares": 2500,
                        "min_a": 1000,
                        "min_b": 1500
                    }
                ]
            }"#,
        );

        let result = load_trade_records_json(&path);

        cleanup(&path);

        let records = result.expect("wrapped JSON should parse successfully");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp, 2000);
        assert_eq!(records[0].label.as_deref(), Some("json-wrapper"));

        match &records[0].action {
            TradeAction::RemoveLiquidity {
                shares,
                min_a,
                min_b,
            } => {
                assert_eq!(*shares, 2500);
                assert_eq!(*min_a, 1000);
                assert_eq!(*min_b, 1500);
            }
            other => panic!("expected RemoveLiquidity, got {other:?}"),
        }
    }

    #[test]
    fn load_trade_records_json_malformed_input_returns_json_error() {
        let path = write_temp_file(
            "json",
            r#"{
                "trades": [
                    {
                        "timestamp": 3000,
                        "action":
                    }
                ]
            "#,
        );

        let result = load_trade_records_json(&path);

        cleanup(&path);

        match result {
            Err(SimulationError::Json {
                path: error_path, ..
            }) => {
                assert!(error_path.ends_with(".json"));
            }
            other => panic!("expected JSON error, got {other:?}"),
        }
    }

    /// Every `TradeAction` variant must survive `save_json_pretty` ->
    /// `load_trade_records`. `TradeRecord` serializes through a derived
    /// `#[serde(flatten)]` impl but deserializes through `FlatTradeRecord`, so
    /// this is what keeps the two halves of the format in agreement.
    #[test]
    fn trade_records_round_trip_through_json() {
        let records = vec![
            TradeRecord {
                timestamp: 1,
                label: Some("swap-in".into()),
                action: TradeAction::SwapExactIn {
                    token_in: "XLM".into(),
                    amount_in: 5_000,
                    min_out: 4_900,
                },
            },
            TradeRecord {
                timestamp: 2,
                label: None,
                action: TradeAction::SwapExactOut {
                    token_out: "USDC".into(),
                    amount_out: 3_000,
                    max_in: Some(3_200),
                },
            },
            TradeRecord {
                timestamp: 3,
                label: Some("deposit".into()),
                action: TradeAction::AddLiquidity {
                    amount_a: 10_000,
                    amount_b: 20_000,
                    min_shares: 9_000,
                },
            },
            TradeRecord {
                timestamp: 4,
                label: None,
                action: TradeAction::RemoveLiquidity {
                    shares: 2_500,
                    min_a: 2_000,
                    min_b: 3_000,
                },
            },
        ];

        let path = temp_file("json");
        save_json_pretty(&path, &records).expect("records should serialize");
        let loaded = load_trade_records(&path);
        cleanup(&path);

        assert_eq!(loaded.expect("round trip should parse"), records);
    }
}
