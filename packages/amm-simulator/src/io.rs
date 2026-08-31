use crate::error::{Result, SimulationError};
use crate::pool::PoolState;
use crate::replay::{TradeAction, TradeRecord};
use serde::Serialize;
use std::fs;
use std::path::Path;

pub fn load_pool_state(path: impl AsRef<Path>) -> Result<PoolState> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| SimulationError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let pool: PoolState = serde_json::from_str(&contents).map_err(|source| SimulationError::Json {
        path: path.display().to_string(),
        source,
    })?;
    pool.validate()?;
    Ok(pool)
}

pub fn load_trade_records(path: impl AsRef<Path>) -> Result<Vec<TradeRecord>> {
    let path = path.as_ref();
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or_default() {
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
    #[derive(serde::Deserialize)]
    struct Row {
        timestamp: u64,
        kind: String,
        label: Option<String>,
        token_in: Option<String>,
        token_out: Option<String>,
        amount_in: Option<i128>,
        amount_out: Option<i128>,
        amount_a: Option<i128>,
        amount_b: Option<i128>,
        shares: Option<i128>,
        min_out: Option<i128>,
        max_in: Option<i128>,
        min_shares: Option<i128>,
        min_a: Option<i128>,
        min_b: Option<i128>,
    }

    let mut reader = csv::Reader::from_path(path).map_err(|source| SimulationError::Csv {
        path: path.display().to_string(),
        source,
    })?;
    let mut records = Vec::new();

    for row in reader.deserialize::<Row>() {
        let row = row.map_err(|source| SimulationError::Csv {
            path: path.display().to_string(),
            source,
        })?;
        let action = match row.kind.as_str() {
            "swap_exact_in" => TradeAction::SwapExactIn {
                token_in: row.token_in.unwrap_or_default(),
                amount_in: row.amount_in.unwrap_or_default(),
                min_out: row.min_out.unwrap_or_default(),
            },
            "swap_exact_out" => TradeAction::SwapExactOut {
                token_out: row.token_out.unwrap_or_default(),
                amount_out: row.amount_out.unwrap_or_default(),
                max_in: row.max_in,
            },
            "add_liquidity" => TradeAction::AddLiquidity {
                amount_a: row.amount_a.unwrap_or_default(),
                amount_b: row.amount_b.unwrap_or_default(),
                min_shares: row.min_shares.unwrap_or_default(),
            },
            "remove_liquidity" => TradeAction::RemoveLiquidity {
                shares: row.shares.unwrap_or_default(),
                min_a: row.min_a.unwrap_or_default(),
                min_b: row.min_b.unwrap_or_default(),
            },
            other => {
                return Err(SimulationError::InvalidInput(format!(
                    "unknown trade kind `{other}`"
                )))
            }
        };

        records.push(TradeRecord {
            timestamp: row.timestamp,
            label: row.label,
            action,
        });
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::TradeAction;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(extension: &str) -> std::path::PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX epoch")
            .as_nanos();

        std::env::temp_dir().join(format!(
            "amm-simulator-io-test-{}-{}.{}",
            std::process::id(),
            timestamp,
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
             100,swap_exact_in,swap-a,XLM,USDC,5000,, , , ,4900,,,,\n",
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
             200,swap_exact_out,swap-b,,USDC,,3000,,,,,3200,,,,\n",
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
             300,add_liquidity,deposit,,,,,10000,20000,, , ,9000,,\n",
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
             400,remove_liquidity,withdraw,,,,,,,2500,,, ,2000,3000\n",
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
             601,swap_exact_out,, ,USDC,,,,,,,,,,,\n\
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
                    "action": {
                        "SwapExactIn": {
                            "token_in": "XLM",
                            "amount_in": 5000,
                            "min_out": 4500
                        }
                    }
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
                        "action": {
                            "RemoveLiquidity": {
                                "shares": 2500,
                                "min_a": 1000,
                                "min_b": 1500
                            }
                        }
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
            Err(SimulationError::Json { path: error_path, .. }) => {
                assert!(error_path.ends_with(".json"));
            }
            other => panic!("expected JSON error, got {other:?}"),
        }
    }
}
