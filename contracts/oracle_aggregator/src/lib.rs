#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype,
    panic_with_error, symbol_short, Address, Env, Vec,
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
}

// ── Storage ────────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    MaxStaleness,
    Sources,
}

pub const MIN_VALID_SOURCES: u32 = 2;

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

        if sources.len() == 0 {
            return AggregatedPrice { price: 0, confidence: 0 };
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

            let client = OracleSourceAdapterClient::new(env, &source.source_contract);

            let (price, source_timestamp) = client.quote(&token_a, &token_b);

            let is_fresh = source_timestamp > 0
                && source_timestamp <= now
                && now - source_timestamp <= max_staleness;

            let mut contributed = false;

            if price > 0 && is_fresh {
                source.last_updated_at = source_timestamp;
                prices.push_back(price);
                contributed = true;
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
            return AggregatedPrice { price: 0, confidence: 0 };
        }

        if persist_sources {
            env.storage().instance().set(&DataKey::Sources, &updated);
        }

        let median = median_i128(env, &prices);

        AggregatedPrice {
            price: median,
            confidence: prices.len(),
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
