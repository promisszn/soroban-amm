use std::{env as std_env, fs, process};

use amm::{AmmPool, AmmPoolClient};
use batch_auction::{BatchAuction, BatchAuctionClient};
use concentrated_liquidity::{ConcentratedLiquidity, ConcentratedLiquidityClient};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient as StellarTokenClient},
    Address, Bytes, Env, String as SorobanString,
};
use token::{LpToken, LpTokenClient};

const REGRESSION_BPS: u128 = 500;
const BASELINE_PATH: &str = "benches/baseline.json";

#[derive(Clone)]
struct Metric {
    name: &'static str,
    cpu_instructions: u64,
    mem_bytes: u64,
}

#[contracttype]
enum ReceiverDataKey {
    Amm,
    TokenA,
    TokenB,
    ShouldRepay,
}

#[contract]
struct BenchFlashLoanReceiver;

#[contractimpl]
impl BenchFlashLoanReceiver {
    pub fn initialize(
        env: Env,
        amm: Address,
        token_a: Address,
        token_b: Address,
        should_repay: bool,
    ) {
        env.storage().instance().set(&ReceiverDataKey::Amm, &amm);
        env.storage()
            .instance()
            .set(&ReceiverDataKey::TokenA, &token_a);
        env.storage()
            .instance()
            .set(&ReceiverDataKey::TokenB, &token_b);
        env.storage()
            .instance()
            .set(&ReceiverDataKey::ShouldRepay, &should_repay);
    }

    pub fn on_flash_loan(
        env: Env,
        amount_a: i128,
        amount_b: i128,
        fee_a: i128,
        fee_b: i128,
        _data: Bytes,
    ) -> bool {
        let should_repay = env
            .storage()
            .instance()
            .get(&ReceiverDataKey::ShouldRepay)
            .unwrap_or(false);
        if should_repay {
            let amm: Address = env.storage().instance().get(&ReceiverDataKey::Amm).unwrap();

            if amount_a > 0 || fee_a > 0 {
                let token_a: Address = env
                    .storage()
                    .instance()
                    .get(&ReceiverDataKey::TokenA)
                    .unwrap();
                StellarTokenClient::new(&env, &token_a).transfer(
                    &env.current_contract_address(),
                    &amm,
                    &(amount_a + fee_a),
                );
            }
            if amount_b > 0 || fee_b > 0 {
                let token_b: Address = env
                    .storage()
                    .instance()
                    .get(&ReceiverDataKey::TokenB)
                    .unwrap();
                StellarTokenClient::new(&env, &token_b).transfer(
                    &env.current_contract_address(),
                    &amm,
                    &(amount_b + fee_b),
                );
            }
        }
        true
    }
}

fn main() {
    let args: Vec<String> = std_env::args().collect();
    let metrics = run_all();
    let json = render_json(&metrics);

    if args.iter().any(|arg| arg == "--write-baseline") {
        fs::write(BASELINE_PATH, json).expect("write baseline");
        return;
    }

    println!("{json}");

    if args.iter().any(|arg| arg == "--check") {
        let baseline = fs::read_to_string(BASELINE_PATH).expect("read benches/baseline.json");
        if let Err(message) = check_regressions(&metrics, &baseline) {
            eprintln!("{message}");
            process::exit(1);
        }
    }
}

fn run_all() -> Vec<Metric> {
    vec![
        measure("amm.swap", bench_amm_swap),
        measure("amm.add_liquidity", bench_amm_add_liquidity),
        measure("amm.remove_liquidity", bench_amm_remove_liquidity),
        measure("amm.flash_loan", bench_amm_flash_loan),
        measure("cl.mint_position", bench_cl_mint_position),
        measure("cl.swap", bench_cl_swap),
        measure("batch.settle_batch", bench_batch_settle),
    ]
}

fn measure(name: &'static str, f: fn(&Env)) -> Metric {
    eprintln!("running {name}");
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    f(&env);
    let (cpu_instructions, mem_bytes) = parse_budget(&format!("{}", env.budget()));
    Metric {
        name,
        cpu_instructions,
        mem_bytes,
    }
}

fn setup_amm(env: &Env, flash_fee_bps: i128) -> (AmmPoolClient<'_>, Address, Address, Address) {
    let admin = Address::generate(env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let lp = env.register_contract(None, LpToken);
    let amm = env.register_contract(None, AmmPool);
    LpTokenClient::new(env, &lp).initialize(
        &amm,
        &SorobanString::from_str(env, "LP"),
        &SorobanString::from_str(env, "LP"),
        &7,
    );
    let client = AmmPoolClient::new(env, &amm);
    client.initialize_with_flash_loan_fee(
        &admin,
        &token_a,
        &token_b,
        &lp,
        &30,
        &admin,
        &0,
        &flash_fee_bps,
    );

    let provider = Address::generate(env);
    StellarAssetClient::new(env, &token_a).mint(&provider, &2_000_000);
    StellarAssetClient::new(env, &token_b).mint(&provider, &2_000_000);
    client.add_liquidity(&provider, &1_000_000, &1_000_000, &0, &u64::MAX);
    (client, amm, token_a, token_b)
}

fn bench_amm_swap(env: &Env) {
    let (client, _, token_a, _) = setup_amm(env, 0);
    let trader = Address::generate(env);
    StellarAssetClient::new(env, &token_a).mint(&trader, &100_000);
    env.budget().reset_unlimited();
    client.swap(&trader, &token_a, &100_000, &0, &u64::MAX);
}

fn bench_amm_add_liquidity(env: &Env) {
    let (client, _, token_a, token_b) = setup_amm(env, 0);
    let provider = Address::generate(env);
    StellarAssetClient::new(env, &token_a).mint(&provider, &500_000);
    StellarAssetClient::new(env, &token_b).mint(&provider, &500_000);
    env.budget().reset_default();
    client.add_liquidity(&provider, &500_000, &500_000, &0, &u64::MAX);
}

fn bench_amm_remove_liquidity(env: &Env) {
    let (client, _, _, _) = setup_amm(env, 0);
    let provider = Address::generate(env);
    let info = client.get_info();
    StellarAssetClient::new(env, &info.token_a).mint(&provider, &500_000);
    StellarAssetClient::new(env, &info.token_b).mint(&provider, &500_000);
    let shares = client.add_liquidity(&provider, &500_000, &500_000, &0, &u64::MAX);
    env.budget().reset_default();
    client.remove_liquidity(&provider, &(shares / 2), &0, &0, &u64::MAX);
}

fn bench_amm_flash_loan(env: &Env) {
    let (client, amm, token_a, token_b) = setup_amm(env, 50);
    let receiver_addr = env.register_contract(None, BenchFlashLoanReceiver);
    let receiver = BenchFlashLoanReceiverClient::new(env, &receiver_addr);
    receiver.initialize(&amm, &token_a, &token_b, &true);
    StellarAssetClient::new(env, &token_a).mint(&receiver_addr, &1_000);
    env.budget().reset_default();
    client.flash_loan(&receiver_addr, &100_000_i128, &0_i128, &Bytes::new(env));
}

fn setup_cl(env: &Env) -> (ConcentratedLiquidityClient<'_>, Address, Address, Address) {
    let admin = Address::generate(env);
    let provider = Address::generate(env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let cl = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(env, &cl);
    client.initialize(&admin, &token_a, &token_b, &30, &0, &1);
    StellarAssetClient::new(env, &token_a).mint(&provider, &10_000_000);
    StellarAssetClient::new(env, &token_b).mint(&provider, &10_000_000);
    (client, provider, token_a, token_b)
}

fn bench_cl_mint_position(env: &Env) {
    let (client, provider, _, _) = setup_cl(env);
    env.budget().reset_default();
    client.mint_position(&provider, &-100, &100, &100_000, &100_000, &0, &0, &u64::MAX);
}

fn bench_cl_swap(env: &Env) {
    let (client, provider, token_a, _) = setup_cl(env);
    client.mint_position(&provider, &-100, &100, &100_000, &100_000, &0, &0, &u64::MAX);
    StellarAssetClient::new(env, &token_a).mint(&provider, &100);
    env.budget().reset_default();
    client.swap(&provider, &true, &100, &0, &0, &u64::MAX);
}

fn bench_batch_settle(env: &Env) {
    env.ledger().set_timestamp(1_000);
    let auction_addr = env.register_contract(None, BatchAuction);
    let admin = Address::generate(env);
    let auction = BatchAuctionClient::new(env, &auction_addr);
    auction.initialize(&admin, &30);
    env.ledger().set_timestamp(1_031);
    env.budget().reset_default();
    let _ = auction.try_settle_batch();
}

fn parse_budget(text: &str) -> (u64, u64) {
    let mut cpu = 0;
    let mut mem = 0;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Cpu limit:") {
            let mut used = rest.split("used:");
            let _ = used.next();
            cpu = used
                .next()
                .and_then(|s| s.split(';').next())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("Mem limit:") {
            let mut used = rest.split("used:");
            let _ = used.next();
            mem = used
                .next()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);
        }
    }
    (cpu, mem)
}

fn render_json(metrics: &[Metric]) -> String {
    let mut out = String::from("{\n  \"metrics\": [\n");
    for (idx, metric) in metrics.iter().enumerate() {
        let comma = if idx + 1 == metrics.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{ \"name\": \"{}\", \"cpu_instructions\": {}, \"mem_bytes\": {} }}{}\n",
            metric.name, metric.cpu_instructions, metric.mem_bytes, comma
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

fn check_regressions(metrics: &[Metric], baseline: &str) -> Result<(), String> {
    for metric in metrics {
        let cpu = read_baseline_value(baseline, metric.name, "cpu_instructions")
            .ok_or_else(|| format!("missing baseline CPU metric for {}", metric.name))?;
        let mem = read_baseline_value(baseline, metric.name, "mem_bytes")
            .ok_or_else(|| format!("missing baseline memory metric for {}", metric.name))?;
        assert_within(
            metric.name,
            "cpu_instructions",
            metric.cpu_instructions,
            cpu,
        )?;
        assert_within(metric.name, "mem_bytes", metric.mem_bytes, mem)?;
    }
    Ok(())
}

fn assert_within(name: &str, key: &str, current: u64, baseline: u64) -> Result<(), String> {
    let allowed = (baseline as u128) * (10_000 + REGRESSION_BPS) / 10_000;
    if (current as u128) > allowed {
        return Err(format!(
            "{name} {key} regressed: current {current}, baseline {baseline}, allowed {allowed}\n\
             \n\
             If this increase is expected (e.g. a storage/TTL change to a hot-path\n\
             contract), regenerate the baseline and commit it in THIS pull request:\n\
             \x20   cargo run -p benches -- --write-baseline\n\
             Otherwise, your change made {name} slower than intended — investigate before merging."
        ));
    }
    Ok(())
}

fn read_baseline_value(text: &str, name: &str, key: &str) -> Option<u64> {
    let name_pos = text.find(&format!("\"name\": \"{name}\""))?;
    let after_name = &text[name_pos..];
    let key_pos = after_name.find(&format!("\"{key}\":"))?;
    let after_key = &after_name[key_pos + key.len() + 3..];
    let digits: String = after_key
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}
