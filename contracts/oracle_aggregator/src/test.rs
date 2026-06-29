#![cfg(test)]

extern crate std;

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Env,
};

use super::*;

// ── Mock adapter ────────────────────────────────────────────────────────────
//
// A configurable adapter contract used as the registered source in
// every test. `quote()` reads a per-instance `price` from storage so
// individual tests can dial each source independently.

#[contract]
struct MockAdapter;

const PRICE_KEY: &str = "price";

#[contractimpl]
impl MockAdapter {
    pub fn set_price(env: Env, price: i128) {
        env.storage()
            .instance()
            .set(&soroban_sdk::symbol_short!("price"), &price);
    }

    pub fn quote(env: Env, _token_a: Address, _token_b: Address) -> i128 {
        env.storage()
            .instance()
            .get(&soroban_sdk::symbol_short!("price"))
            .unwrap_or(0)
    }
}

struct Harness<'a> {
    env: Env,
    aggregator: OracleAggregatorClient<'a>,
    admin: Address,
    token_a: Address,
    token_b: Address,
}

fn deploy(env: &Env, max_staleness: u64) -> Harness<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let aggregator_id = env.register_contract(None, OracleAggregator);
    let aggregator = OracleAggregatorClient::new(env, &aggregator_id);
    aggregator.initialize(&admin, &max_staleness, &0u32); // deviation filtering disabled by default
    let token_a = Address::generate(env);
    let token_b = Address::generate(env);
    Harness {
        env: env.clone(),
        aggregator,
        admin,
        token_a,
        token_b,
    }
}

fn deploy_source(env: &Env, price: i128) -> Address {
    let id = env.register_contract(None, MockAdapter);
    let client = MockAdapterClient::new(env, &id);
    client.set_price(&price);
    id
}

fn set_now(env: &Env, ts: u64) {
    env.ledger().set(LedgerInfo {
        timestamp: ts,
        protocol_version: 22,
        sequence_number: 0,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1,
        min_persistent_entry_ttl: 1,
        max_entry_ttl: 6_312_000,
    });
}

#[test]
fn initialize_seeds_admin_and_staleness() {
    let env = Env::default();
    let h = deploy(&env, 600);
    assert_eq!(h.aggregator.get_admin(), h.admin);
    assert_eq!(h.aggregator.get_max_staleness(), 600);
    assert_eq!(h.aggregator.get_max_deviation_bps(), 0);
    assert_eq!(h.aggregator.list_sources().len(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn initialize_is_one_time() {
    let env = Env::default();
    let h = deploy(&env, 600);
    h.aggregator.initialize(&h.admin, &600, &0u32);
}

#[test]
fn register_source_appends_to_registry() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 110);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::External);
    let sources = h.aggregator.list_sources();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources.get_unchecked(0).source_contract, s1);
    assert_eq!(sources.get_unchecked(1).source_contract, s2);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn register_source_rejects_duplicates() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
}

#[test]
fn remove_source_drops_the_entry() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 110);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::External);
    h.aggregator.remove_source(&h.admin, &s1);
    let sources = h.aggregator.list_sources();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources.get_unchecked(0).source_contract, s2);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn remove_source_panics_on_unknown_address() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let ghost = Address::generate(&env);
    h.aggregator.remove_source(&h.admin, &ghost);
}

#[test]
fn get_price_returns_median_of_three_sources() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 110);
    let s3 = deploy_source(&env, 150);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::ClTwap);
    h.aggregator
        .register_source(&h.admin, &s3, &OracleSourceType::External);
    set_now(&env, 1_000);

    let result = h.aggregator.get_price(&h.token_a, &h.token_b);
    assert_eq!(result.price, 110);
    assert_eq!(result.confidence, 3);
}

#[test]
fn get_price_returns_two_way_median_average() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 200);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::External);
    set_now(&env, 1_000);
    let result = h.aggregator.get_price(&h.token_a, &h.token_b);
    assert_eq!(result.price, 150);
    assert_eq!(result.confidence, 2);
}

#[test]
fn stale_source_excluded_after_window() {
    let env = Env::default();
    // Use max_staleness=600 so the 200s advance doesn't expire s1 or s3.
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 110);
    let s3 = deploy_source(&env, 150);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::ClTwap);
    h.aggregator
        .register_source(&h.admin, &s3, &OracleSourceType::External);

    set_now(&env, 1_000);
    h.aggregator.get_price(&h.token_a, &h.token_b);

    // Source-2 stops reporting (price goes to 0 → not counted).
    let s2_client = MockAdapterClient::new(&env, &s2);
    s2_client.set_price(&0);

    // Advance the clock; s1 and s3 last reported at t=1000, 200s ago, still within 600s window.
    set_now(&env, 1_000 + 200);

    let result = h.aggregator.get_price(&h.token_a, &h.token_b);
    // s2 excluded (price=0 → not counted); s1 + s3 remain.
    // Median of (100, 150) = 125.
    assert_eq!(result.price, 125);
    assert_eq!(result.confidence, 2);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn single_source_below_min_panics() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let s1 = deploy_source(&env, 100);
    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    set_now(&env, 1_000);
    h.aggregator.get_price(&h.token_a, &h.token_b);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn empty_registry_panics() {
    let env = Env::default();
    let h = deploy(&env, 600);
    set_now(&env, 1_000);
    h.aggregator.get_price(&h.token_a, &h.token_b);
}

#[test]
fn set_max_staleness_updates_window() {
    let env = Env::default();
    let h = deploy(&env, 600);
    h.aggregator.set_max_staleness(&h.admin, &120);
    assert_eq!(h.aggregator.get_max_staleness(), 120);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn set_max_staleness_rejects_zero() {
    let env = Env::default();
    let h = deploy(&env, 600);
    h.aggregator.set_max_staleness(&h.admin, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn register_source_rejects_non_admin() {
    let env = Env::default();
    let h = deploy(&env, 600);
    let attacker = Address::generate(&env);
    let s1 = deploy_source(&env, 100);
    h.aggregator
        .register_source(&attacker, &s1, &OracleSourceType::AmmTwap);
}

#[test]
fn stale_src_event_emitted_when_sources_skipped() {
    use soroban_sdk::testutils::Events as _;
    use soroban_sdk::IntoVal;

    let env = Env::default();
    let h = deploy(&env, 60);

    let s1 = deploy_source(&env, 100); // healthy
    let s2 = deploy_source(&env, 0);   // price=0 → skipped
    let s3 = deploy_source(&env, 0);   // price=0 → skipped

    h.aggregator
        .register_source(&h.admin, &s1, &OracleSourceType::AmmTwap);
    h.aggregator
        .register_source(&h.admin, &s2, &OracleSourceType::ClTwap);
    h.aggregator
        .register_source(&h.admin, &s3, &OracleSourceType::External);

    set_now(&env, 1_000);

    // get_price_safe won't panic even though only 1 source is valid.
    h.aggregator.get_price_safe(&h.token_a, &h.token_b);

    let events = env.events().all();
    let agg_id = h.aggregator.address.clone();
    let expected_topics: soroban_sdk::Vec<soroban_sdk::Val> =
        (symbol_short!("stale_src"),).into_val(&env);

    let stale_event = events
        .iter()
        .find(|e| e.0 == agg_id && e.1 == expected_topics)
        .expect("stale_src event must be emitted when sources are skipped");

    // Data is a tuple wrapping a Vec<Address> of the two skipped sources.
    let (stale_addrs,): (soroban_sdk::Vec<Address>,) = stale_event.2.into_val(&env);
    assert_eq!(stale_addrs.len(), 2);
    assert!(stale_addrs.contains(&s2));
    assert!(stale_addrs.contains(&s3));
}

// ── Deviation-bound tests ───────────────────────────────────────────────────

#[test]
fn outlier_excluded_from_confidence() {
    // Three sources: 100, 105, 1_000_000 (wildly off).
    // Median = 105; max_deviation_bps = 500 (5%).
    // 100: |100-105|/105 = 4.76% → within band.
    // 105: deviation = 0 → within band.
    // 1_000_000: deviation >> 5% → outside band.
    // Agreeing = 2, so confidence = 2 (not 3).
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let agg_id = env.register_contract(None, OracleAggregator);
    let agg = OracleAggregatorClient::new(&env, &agg_id);
    agg.initialize(&admin, &600u64, &500u32); // 5% band
    let token_a = Address::generate(&env);
    let token_b = Address::generate(&env);

    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 105);
    let s3 = deploy_source(&env, 1_000_000);
    agg.register_source(&admin, &s1, &OracleSourceType::AmmTwap);
    agg.register_source(&admin, &s2, &OracleSourceType::ClTwap);
    agg.register_source(&admin, &s3, &OracleSourceType::External);
    set_now(&env, 1_000);

    let result = agg.get_price(&token_a, &token_b);
    assert_eq!(result.price, 105); // median of (100, 105, 1_000_000)
    assert_eq!(result.confidence, 2); // s3 excluded
}

#[test]
fn set_max_deviation_bps_updates_and_is_readable() {
    let env = Env::default();
    let h = deploy(&env, 600);
    assert_eq!(h.aggregator.get_max_deviation_bps(), 0);
    h.aggregator.set_max_deviation_bps(&h.admin, &200u32);
    assert_eq!(h.aggregator.get_max_deviation_bps(), 200);
}

#[test]
fn deviation_bound_zero_disables_filtering() {
    // With max_deviation_bps = 0 the outlier must still count.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let agg_id = env.register_contract(None, OracleAggregator);
    let agg = OracleAggregatorClient::new(&env, &agg_id);
    agg.initialize(&admin, &600u64, &0u32); // filtering disabled
    let token_a = Address::generate(&env);
    let token_b = Address::generate(&env);

    let s1 = deploy_source(&env, 100);
    let s2 = deploy_source(&env, 105);
    let s3 = deploy_source(&env, 1_000_000);
    agg.register_source(&admin, &s1, &OracleSourceType::AmmTwap);
    agg.register_source(&admin, &s2, &OracleSourceType::ClTwap);
    agg.register_source(&admin, &s3, &OracleSourceType::External);
    set_now(&env, 1_000);

    let result = agg.get_price(&token_a, &token_b);
    assert_eq!(result.price, 105);
    assert_eq!(result.confidence, 3); // all three sources counted
}
