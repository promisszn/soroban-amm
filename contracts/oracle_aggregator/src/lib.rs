//! Multi-source price oracle aggregator (#304).
//!
//! Pulls prices from registered sources (AMM TWAP, CL TWAP, external
//! feeds) and returns a **median** composite price plus a confidence
//! score = number of fresh, agreeing sources.
//!
//! ## Why median + minimum-sources
//!
//! - Median is resistant to a single manipulated source (one bad
//!   price out of three changes the median by at most the gap
//!   between the bad price and the second-best one — but the
//!   second-best price's vote dominates).
//! - Requiring `>= MIN_VALID_SOURCES` (2) prevents the aggregator
//!   from reporting prices when only one feed is up.
//! - Staleness gating excludes sources whose `last_updated_at`
//!   has aged past `max_staleness_seconds`, even if they're listed
//!   in the registry.
//!
//! ## Adapter shape
//!
//! Each registered source contract must implement a uniform
//! `quote(token_a, token_b) -> i128` reader. AMM TWAP / CL TWAP
//! integrators deploy a tiny **adapter** wrapper contract that
//! resolves the (token_a, token_b) pair to the correct pool /
//! window and forwards to the underlying source. The `OracleSourceType`
//! enum captures the *kind* of source for indexer / dashboard
//! metadata but the aggregator itself is source-type-agnostic.

#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype,
    panic_with_error, symbol_short, Address, Env, Vec,
};

// ── Public types ────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleSourceType {
    /// AMM TWAP — adapter wraps `twap_consumer::get_twap_price`.
    AmmTwap = 0,
    /// Concentrated-liquidity TWAP — adapter wraps `cl::observe`.
    ClTwap = 1,
    /// External feed (e.g. Reflector, Pyth, Chainlink bridge).
    External = 2,
}

#[contracttype]
#[derive(Clone)]
pub struct OracleSource {
    /// Adapter contract address — must implement
    /// `quote(token_a, token_b) -> i128`.
    pub source_contract: Address,
    pub source_type: OracleSourceType,
    /// Last ledger timestamp at which the aggregator successfully
    /// read a positive price from this source. 0 means "never read".
    pub last_updated_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct AggregatedPrice {
    pub price: i128,
    /// Number of fresh sources that contributed. Always `>=
    /// MIN_VALID_SOURCES` on a successful return.
    pub confidence: u32,
}

#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum OracleError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    SourceAlreadyRegistered = 4,
    SourceNotFound = 5,
    InsufficientSources = 6,
    InvalidStaleness = 7,
    /// Returned (internally) when a source's price lies outside the
    /// configured deviation band around the median. Not panicked
    /// directly — sources are silently dropped from confidence.
    OutsideDeviationBand = 8,
}

// ── Storage layout ──────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    /// Cap on a source's age (seconds) before it's excluded from the
    /// aggregation.
    MaxStaleness,
    /// Vec<OracleSource>, append-only via `register_source`, trimmed
    /// by `remove_source`.
    Sources,
    /// Maximum allowed deviation from the median in basis points.
    /// A source whose price deviates by more than this fraction is
    /// excluded from confidence. `0` disables the filter (every
    /// positive, fresh price counts regardless of spread).
    MaxDeviationBps,
}

pub const MIN_VALID_SOURCES: u32 = 2;

// ── Adapter client ──────────────────────────────────────────────────────────
//
// Every registered source must implement this one-function adapter
// surface. AMM TWAP / CL TWAP / external feed wrappers all reduce to
// the same shape so the aggregator stays agnostic of source internals.

#[contractclient(name = "OracleSourceAdapterClient")]
pub trait OracleSourceAdapter {
    fn quote(env: Env, token_a: Address, token_b: Address) -> i128;
}

// ── Contract ────────────────────────────────────────────────────────────────

#[contract]
pub struct OracleAggregator;

#[contractimpl]
impl OracleAggregator {
    /// One-time initialization. Stores admin and the global staleness
    /// cap. Panics if already initialized so an upgrade must go
    /// through a separate path explicitly.
    pub fn initialize(env: Env, admin: Address, max_staleness_seconds: u64, max_deviation_bps: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, OracleError::AlreadyInitialized);
        }
        if max_staleness_seconds == 0 {
            panic_with_error!(&env, OracleError::InvalidStaleness);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &max_staleness_seconds);
        env.storage()
            .instance()
            .set(&DataKey::MaxDeviationBps, &max_deviation_bps);
        let empty: Vec<OracleSource> = Vec::new(&env);
        env.storage().instance().set(&DataKey::Sources, &empty);
    }

    /// Admin-only. Adds `source_contract` to the registry. Re-adding
    /// an already-registered contract reverts so accidental
    /// double-registration can't dilute the median by counting one
    /// source twice.
    pub fn register_source(
        env: Env,
        admin: Address,
        source_contract: Address,
        source_type: OracleSourceType,
    ) {
        require_admin(&env, &admin);
        let mut sources = read_sources(&env);
        for i in 0..sources.len() {
            let existing = sources.get_unchecked(i);
            if existing.source_contract == source_contract {
                panic_with_error!(&env, OracleError::SourceAlreadyRegistered);
            }
        }
        sources.push_back(OracleSource {
            source_contract: source_contract.clone(),
            source_type,
            last_updated_at: 0,
        });
        env.storage().instance().set(&DataKey::Sources, &sources);

        env.events().publish(
            (symbol_short!("src_reg"), admin),
            (source_contract, source_type as u32),
        );
    }

    /// Admin-only. Removes a registered source.
    pub fn remove_source(env: Env, admin: Address, source_contract: Address) {
        require_admin(&env, &admin);
        let sources = read_sources(&env);
        let mut next: Vec<OracleSource> = Vec::new(&env);
        let mut found = false;
        for i in 0..sources.len() {
            let s = sources.get_unchecked(i);
            if s.source_contract == source_contract {
                found = true;
            } else {
                next.push_back(s);
            }
        }
        if !found {
            panic_with_error!(&env, OracleError::SourceNotFound);
        }
        env.storage().instance().set(&DataKey::Sources, &next);

        env.events()
            .publish((symbol_short!("src_rm"), admin), (source_contract,));
    }

    /// Admin-only. Update the staleness cap.
    pub fn set_max_staleness(env: Env, admin: Address, max_staleness_seconds: u64) {
        require_admin(&env, &admin);
        if max_staleness_seconds == 0 {
            panic_with_error!(&env, OracleError::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &max_staleness_seconds);
    }

    /// Admin-only. Update the maximum allowed deviation from the
    /// median (in basis points). Set to `0` to disable filtering —
    /// every fresh, positive price counts toward confidence regardless
    /// of how far it deviates from the median.
    pub fn set_max_deviation_bps(env: Env, admin: Address, max_deviation_bps: u32) {
        require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MaxDeviationBps, &max_deviation_bps);
    }

    /// Query every fresh source, return the median + confidence
    /// (number of fresh sources that contributed). Reverts with
    /// `InsufficientSources` when fewer than `MIN_VALID_SOURCES`
    /// produced a positive price within the staleness window.
    pub fn get_price(env: Env, token_a: Address, token_b: Address) -> AggregatedPrice {
        let result = Self::aggregate_price(&env, token_a.clone(), token_b.clone(), true);
        if result.confidence == 0 {
            panic_with_error!(&env, OracleError::InsufficientSources);
        }
        env.events().publish(
            (symbol_short!("price"),),
            (token_a, token_b, result.price, result.confidence),
        );
        result
    }

    /// Non-panicking price query for pool circuit breakers (#318).
    /// Returns `confidence = 0` when all sources are stale or insufficient.
    pub fn get_price_safe(env: Env, token_a: Address, token_b: Address) -> AggregatedPrice {
        Self::aggregate_price(&env, token_a, token_b, false)
    }

    fn aggregate_price(
        env: &Env,
        token_a: Address,
        token_b: Address,
        persist_sources: bool,
    ) -> AggregatedPrice {
        let sources = read_sources(env);
        if sources.len() == 0 {
            return AggregatedPrice {
                price: 0,
                confidence: 0,
            };
        }

        let now = env.ledger().timestamp();
        let max_staleness: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap_or(0);

        let mut prices: Vec<i128> = Vec::new(env);
        let mut updated: Vec<OracleSource> = Vec::new(env);
        let mut stale_sources: Vec<Address> = Vec::new(env);

        for i in 0..sources.len() {
            let mut source = sources.get_unchecked(i);
            let is_fresh = source.last_updated_at == 0
                || now <= source.last_updated_at
                || now - source.last_updated_at <= max_staleness;

            let mut contributed = false;
            if is_fresh {
                let client = OracleSourceAdapterClient::new(env, &source.source_contract);
                let price = client.quote(&token_a, &token_b);
                if price > 0 {
                    source.last_updated_at = now;
                    prices.push_back(price);
                    contributed = true;
                }
            }
            if !contributed {
                stale_sources.push_back(source.source_contract.clone());
            }
            updated.push_back(source);
        }

        if !stale_sources.is_empty() {
            env.events()
                .publish((symbol_short!("stale_src"),), (stale_sources,));
        }

        if prices.len() < MIN_VALID_SOURCES {
            return AggregatedPrice {
                price: 0,
                confidence: 0,
            };
        }

        if persist_sources {
            env.storage().instance().set(&DataKey::Sources, &updated);
        }

        let median = median_i128(env, &prices);

        // Filter outliers: count only sources whose price falls within
        // `max_deviation_bps` of the median. A value of 0 disables the
        // filter and every fresh, positive price contributes.
        let max_dev: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxDeviationBps)
            .unwrap_or(0);

        let agreeing = if max_dev == 0 {
            prices.len()
        } else {
            let mut count: u32 = 0;
            for i in 0..prices.len() {
                let p = prices.get_unchecked(i);
                // |p - median| * 10_000 <= max_dev * median
                let diff = if p >= median { p - median } else { median - p };
                if diff * 10_000 <= (max_dev as i128) * median {
                    count += 1;
                }
            }
            count
        };

        if agreeing < MIN_VALID_SOURCES {
            return AggregatedPrice {
                price: 0,
                confidence: 0,
            };
        }

        AggregatedPrice {
            price: median,
            confidence: agreeing,
        }
    }

    /// Read-only — list every source currently in the registry.
    pub fn list_sources(env: Env) -> Vec<OracleSource> {
        read_sources(&env)
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap_or(0)
    }

    pub fn get_max_deviation_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxDeviationBps)
            .unwrap_or(0)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, OracleError::NotInitialized))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn read_sources(env: &Env) -> Vec<OracleSource> {
    env.storage()
        .instance()
        .get(&DataKey::Sources)
        .unwrap_or_else(|| panic_with_error!(env, OracleError::NotInitialized))
}

fn require_admin(env: &Env, claimed: &Address) {
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, OracleError::NotInitialized));
    if &admin != claimed {
        panic_with_error!(env, OracleError::NotAdmin);
    }
    claimed.require_auth();
}

/// Pure median over a `soroban_sdk::Vec<i128>`. Insertion sort —
/// cheap for the small source counts (3-10) the aggregator targets.
fn median_i128(env: &Env, values: &Vec<i128>) -> i128 {
    let n = values.len();
    let mut sorted: Vec<i128> = Vec::new(env);
    for i in 0..n {
        let v = values.get_unchecked(i);
        let mut inserted = false;
        let mut next: Vec<i128> = Vec::new(env);
        for j in 0..sorted.len() {
            let s = sorted.get_unchecked(j);
            if !inserted && v < s {
                next.push_back(v);
                inserted = true;
            }
            next.push_back(s);
        }
        if !inserted {
            next.push_back(v);
        }
        sorted = next;
    }
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        let lo = sorted.get_unchecked(mid - 1);
        let hi = sorted.get_unchecked(mid);
        (lo + hi) / 2
    } else {
        sorted.get_unchecked(mid)
    }
}

#[cfg(test)]
mod test;
