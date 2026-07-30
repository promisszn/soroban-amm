#![no_std]
//! Oracle aggregator: reports the median price over **fresh, agreeing**
//! sources.
//!
//! A quote contributes to the result only when it is both fresh (reported
//! within `max_staleness_seconds`) and *agreeing* — within a configurable
//! deviation band (in basis points) of the median of the fresh quotes.
//! Sources outside the band are dropped and reported via a `deviant` event,
//! and only agreeing sources are counted toward `confidence`. This ensures a
//! high confidence score actually implies the sources agree: a single wildly
//! deviating feed can no longer inflate confidence or drag the reported price
//! toward an attacker's value. The band defaults to
//! [`DEFAULT_MAX_DEVIATION_BPS`] and is tunable by the admin via
//! [`OracleAggregator::set_max_deviation_bps`].

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, Address, Env, Vec,
};

// ── Public types ────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleSourceType {
    AmmTwap = 0,
    ClTwap = 1,
    External = 2,
}

#[contracttype]
#[derive(Clone)]
pub struct OracleSource {
    pub source_contract: Address,
    pub source_type: OracleSourceType,
    /// Last timestamp reported by the source itself
    pub last_updated_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct AggregatedPrice {
    pub price: i128,
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
    InvalidDeviation = 8,
}

// ── Storage ────────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MaxStaleness,
    Sources,
    MaxDeviationBps,
}

pub const MIN_VALID_SOURCES: u32 = 2;

/// Basis-points denominator (100% = 10_000 bps).
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Default deviation band: a fresh quote must be within 5% of the median of
/// the fresh quotes to count as agreeing. Chosen tight enough to flag the
/// "pull the price halfway to the attacker" manipulation described in the
/// aggregator's threat model, while tolerating normal cross-venue spread.
pub const DEFAULT_MAX_DEVIATION_BPS: u32 = 500;

// ── Adapter client (UPDATED) ───────────────────────────────────────────────

#[contractclient(name = "OracleSourceAdapterClient")]
pub trait OracleSourceAdapter {
    /// Returns (price, last_updated_timestamp)
    fn quote(env: Env, token_a: Address, token_b: Address) -> (i128, u64);
}

// ── Contract ───────────────────────────────────────────────────────────────

#[contract]
pub struct OracleAggregator;

#[contractimpl]
impl OracleAggregator {
    pub fn initialize(env: Env, admin: Address, max_staleness_seconds: u64) {
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
            .set(&DataKey::MaxDeviationBps, &DEFAULT_MAX_DEVIATION_BPS);

        let empty: Vec<OracleSource> = Vec::new(&env);
        env.storage().instance().set(&DataKey::Sources, &empty);
    }

    pub fn register_source(
        env: Env,
        admin: Address,
        source_contract: Address,
        source_type: OracleSourceType,
    ) {
        require_admin(&env, &admin);

        let mut sources = read_sources(&env);
        for i in 0..sources.len() {
            if sources.get_unchecked(i).source_contract == source_contract {
                panic_with_error!(&env, OracleError::SourceAlreadyRegistered);
            }
        }

        sources.push_back(OracleSource {
            source_contract: source_contract.clone(),
            source_type,
            last_updated_at: 0,
        });

        env.storage().instance().set(&DataKey::Sources, &sources);
    }

    pub fn remove_source(env: Env, admin: Address, source_contract: Address) {
        require_admin(&env, &admin);

        let sources = read_sources(&env);
        let mut new_sources: Vec<OracleSource> = Vec::new(&env);
        let mut found = false;
        for i in 0..sources.len() {
            let source = sources.get_unchecked(i);
            if source.source_contract == source_contract {
                found = true;
            } else {
                new_sources.push_back(source);
            }
        }

        if !found {
            panic_with_error!(&env, OracleError::SourceNotFound);
        }

        if new_sources.len() < MIN_VALID_SOURCES {
            panic_with_error!(&env, OracleError::InsufficientSources);
        }

        env.storage()
            .instance()
            .set(&DataKey::Sources, &new_sources);
    }

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

        if sources.is_empty() {
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
        // Source address paralleling each entry in `prices`, so a deviating
        // quote can be reported by contract address.
        let mut fresh_sources: Vec<Address> = Vec::new(env);
        let mut updated: Vec<OracleSource> = Vec::new(env);
        let mut stale_sources: Vec<Address> = Vec::new(env);

        for i in 0..sources.len() {
            let mut source = sources.get_unchecked(i);

            let client = OracleSourceAdapterClient::new(env, &source.source_contract);

            let (price, source_timestamp) = match client.try_quote(&token_a, &token_b) {
                Ok(Ok(res)) => res,
                _ => (0, 0),
            };

            let is_fresh = source_timestamp > 0
                && source_timestamp <= now
                && now - source_timestamp <= max_staleness;

            let mut contributed = false;

            if price > 0 && is_fresh {
                source.last_updated_at = source_timestamp;
                prices.push_back(price);
                fresh_sources.push_back(source.source_contract.clone());
                contributed = true;
            }

            if !contributed {
                stale_sources.push_back(source.source_contract.clone());
            }

            updated.push_back(source);
        }

        if persist_sources && !stale_sources.is_empty() {
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

        // Reference median over all fresh quotes. The median is robust to a
        // minority of outliers, so it is a sound centre for the agreement test.
        let reference_median = median_i128(env, &prices);
        let max_deviation_bps = read_max_deviation_bps(env);

        // Keep only quotes within the deviation band of the reference median;
        // everything else is a disagreeing outlier that must not inflate
        // confidence or move the reported price.
        let mut agreeing: Vec<i128> = Vec::new(env);
        let mut deviant_sources: Vec<Address> = Vec::new(env);
        for i in 0..prices.len() {
            let price = prices.get_unchecked(i);
            if within_deviation_band(price, reference_median, max_deviation_bps) {
                agreeing.push_back(price);
            } else {
                deviant_sources.push_back(fresh_sources.get_unchecked(i));
            }
        }

        if persist_sources && !deviant_sources.is_empty() {
            env.events()
                .publish((symbol_short!("deviant"),), (deviant_sources,));
        }

        // Too few sources agree — a high-variance quote set. Report no
        // confidence rather than a price that only looks corroborated.
        if agreeing.len() < MIN_VALID_SOURCES {
            return AggregatedPrice {
                price: 0,
                confidence: 0,
            };
        }

        // Report the median of the agreeing subset so a dropped outlier does
        // not skew the result, and count only agreeing sources as confidence.
        let median = median_i128(env, &agreeing);

        AggregatedPrice {
            price: median,
            confidence: agreeing.len(),
        }
    }

    pub fn list_sources(env: Env) -> Vec<OracleSource> {
        read_sources(&env)
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap_or(0)
    }

    pub fn set_max_staleness(env: Env, admin: Address, max_staleness_seconds: u64) {
        require_admin(&env, &admin);
        if max_staleness_seconds == 0 {
            panic_with_error!(&env, OracleError::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &max_staleness_seconds);
    }

    /// Returns the deviation band (in bps) a fresh quote must fall within of
    /// the median to be counted as agreeing.
    pub fn get_max_deviation_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxDeviationBps)
            .unwrap_or(DEFAULT_MAX_DEVIATION_BPS)
    }

    /// Admin: set the agreement deviation band, in bps (must be `1..=10_000`).
    pub fn set_max_deviation_bps(env: Env, admin: Address, max_deviation_bps: u32) {
        require_admin(&env, &admin);
        if max_deviation_bps == 0 || max_deviation_bps as i128 > BPS_DENOMINATOR {
            panic_with_error!(&env, OracleError::InvalidDeviation);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxDeviationBps, &max_deviation_bps);
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, OracleError::NotInitialized))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

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

fn read_max_deviation_bps(env: &Env) -> i128 {
    let bps: u32 = env
        .storage()
        .instance()
        .get(&DataKey::MaxDeviationBps)
        .unwrap_or(DEFAULT_MAX_DEVIATION_BPS);
    bps as i128
}

/// Returns `true` when `price` is within `max_deviation_bps` of `median`.
///
/// Uses the cross-multiplied form `|price - median| * 10_000 <= bps * median`
/// to avoid a division (and its precision loss). `median` is always `> 0`
/// here because it is derived from strictly-positive quotes.
fn within_deviation_band(price: i128, median: i128, max_deviation_bps: i128) -> bool {
    let diff = price - median;
    let abs_diff = if diff < 0 { -diff } else { diff };
    abs_diff.saturating_mul(BPS_DENOMINATOR) <= max_deviation_bps.saturating_mul(median)
}

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

    if sorted.len().is_multiple_of(2) {
        let lo = sorted.get_unchecked(mid - 1);
        let hi = sorted.get_unchecked(mid);
        // Midpoint via `lo + (hi - lo) / 2` rather than `(lo + hi) / 2`.
        // The sum of two individually-valid prices can exceed i128::MAX and
        // overflow (panic in debug, wraparound in release). Since `sorted` is
        // ascending, `hi >= lo`, and aggregated prices are strictly positive,
        // so `hi - lo` stays within range and the midpoint never overflows.
        lo + (hi - lo) / 2
    } else {
        sorted.get_unchecked(mid)
    }
}

#[cfg(test)]
mod test;
