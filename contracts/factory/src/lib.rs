//! Factory contract — deploys and registers AMM pool instances.
//!
//! Flow:
//!   1. Deploy this contract.
//!   2. Call `initialize` with the admin address and pre-uploaded WASM hashes
//!      for the AMM pool and LP token contracts.
//!   3. Call `create_pool` for each token pair you want a pool for.
//!   4. Use `get_pool` / `all_pools` to discover deployed pools.

#![no_std]

use amm::AmmPoolClient;
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Vec};
use token::LpTokenClient;

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Pool(Address, Address), // normalized (token_a, token_b) → pool Address
    AllPools,               // Vec<Address> of every deployed pool
    Admin,
    AmmWasmHash,
    TokenWasmHash,
    PoolCount, // u64 monotonic counter — used to derive unique deploy salts
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct Factory;

#[contractimpl]
impl Factory {
    // ── Setup ─────────────────────────────────────────────────────────────────

    /// One-time factory setup.
    ///
    /// `amm_wasm_hash` and `token_wasm_hash` must be uploaded to the network
    /// (via `stellar contract upload`) before calling this function.
    pub fn initialize(
        env: Env,
        admin: Address,
        amm_wasm_hash: BytesN<32>,
        token_wasm_hash: BytesN<32>,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::AmmWasmHash, &amm_wasm_hash);
        env.storage()
            .instance()
            .set(&DataKey::TokenWasmHash, &token_wasm_hash);
        env.storage()
            .instance()
            .set(&DataKey::AllPools, &Vec::<Address>::new(&env));
        env.storage().instance().set(&DataKey::PoolCount, &0u64);
    }

    // ── Pool creation ─────────────────────────────────────────────────────────

    /// Deploy a new AMM pool for `(token_a, token_b)` with `fee_bps` swap fee.
    ///
    /// Token pair order is normalised — the pool is always stored with the
    /// lexicographically smaller address as `token_a`, so callers do not need
    /// to match the original order when looking up a pool.
    ///
    /// Panics if a pool for this pair already exists.
    pub fn create_pool(env: Env, token_a: Address, token_b: Address, fee_bps: i128) -> Address {
        // Normalise: smaller address is always token_a.
        let (ta, tb) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };

        if env
            .storage()
            .instance()
            .has(&DataKey::Pool(ta.clone(), tb.clone()))
        {
            panic!("pool already exists");
        }

        let amm_wasm: BytesN<32> = env.storage().instance().get(&DataKey::AmmWasmHash).unwrap();
        let token_wasm: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::TokenWasmHash)
            .unwrap();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();

        // Derive two unique salts per pool from a monotonic counter.
        let n: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PoolCount)
            .unwrap_or(0);
        env.storage().instance().set(&DataKey::PoolCount, &(n + 1));

        let lp_salt = Self::make_salt(&env, n * 2);
        let pool_salt = Self::make_salt(&env, n * 2 + 1);

        // Deploy LP token then AMM pool.
        let lp_addr = env
            .deployer()
            .with_current_contract(lp_salt)
            .deploy(token_wasm);
        let pool_addr = env
            .deployer()
            .with_current_contract(pool_salt)
            .deploy(amm_wasm);

        // Initialize LP token — admin must be the pool so it can mint/burn.
        LpTokenClient::new(&env, &lp_addr).initialize(
            &pool_addr,
            &soroban_sdk::String::from_str(&env, "AMM LP Token"),
            &soroban_sdk::String::from_str(&env, "ALP"),
            &7u32,
        );

        // Initialize AMM pool.
        AmmPoolClient::new(&env, &pool_addr).initialize(
            &admin, &ta, &tb, &lp_addr, &fee_bps, &admin,  // fee_recipient
            &0_i128, // protocol_fee_bps (disabled by default)
        );

        // Register pool in both lookup indexes.
        env.storage()
            .instance()
            .set(&DataKey::Pool(ta.clone(), tb.clone()), &pool_addr);

        let mut all: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::AllPools)
            .unwrap_or_else(|| Vec::new(&env));
        all.push_back(pool_addr.clone());
        env.storage().instance().set(&DataKey::AllPools, &all);

        pool_addr
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Return the pool address for `(token_a, token_b)`, or `None` if it does
    /// not exist. Token pair order does not matter.
    pub fn get_pool(env: Env, token_a: Address, token_b: Address) -> Option<Address> {
        let (ta, tb) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };
        env.storage().instance().get(&DataKey::Pool(ta, tb))
    }

    /// Return the addresses of every pool deployed by this factory.
    pub fn all_pools(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::AllPools)
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    /// Build a deterministic 32-byte salt from a u64 index.
    fn make_salt(env: &Env, index: u64) -> BytesN<32> {
        let mut arr = [0u8; 32];
        arr[..8].copy_from_slice(&index.to_be_bytes());
        BytesN::from_array(env, &arr)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────
//
// Tests deploy the AMM and token contracts as real WASM. Build the WASM first:
//
//   cargo build --release --target wasm32v1-none
//
// Then run:
//
//   cargo test -p factory

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    // Embed compiled WASM at test-compile time.
    mod amm_wasm {
        soroban_sdk::contractimport!(file = "../../target/wasm32v1-none/release/amm.wasm");
    }

    mod token_wasm {
        soroban_sdk::contractimport!(file = "../../target/wasm32v1-none/release/token.wasm");
    }

    /// Deploy the factory and return (env, factory_client).
    fn setup() -> (Env, Address, FactoryClient<'static>) {
        // SAFETY: the client borrows from env. We return both so the borrow is valid.
        // Workaround: heap-allocate env and leak. In tests this is fine.
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        // Return env + factory_addr so caller can rebuild the client without
        // lifetime friction.
        (env, factory_addr, factory)
    }

    #[test]
    fn test_create_pool() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        let ta = Address::generate(&env);
        let tb = Address::generate(&env);

        let pool = factory.create_pool(&ta, &tb, &30_i128);

        assert_eq!(factory.get_pool(&ta, &tb), Some(pool.clone()));
        assert_eq!(factory.all_pools().len(), 1);
    }

    #[test]
    fn test_normalize_order() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        let ta = Address::generate(&env);
        let tb = Address::generate(&env);

        factory.create_pool(&ta, &tb, &30_i128);

        // Reverse-order lookup returns the same pool.
        assert_eq!(factory.get_pool(&ta, &tb), factory.get_pool(&tb, &ta));
    }

    #[test]
    fn test_duplicate_pool_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        let ta = Address::generate(&env);
        let tb = Address::generate(&env);

        factory.create_pool(&ta, &tb, &30_i128);
        let result = factory.try_create_pool(&ta, &tb, &30_i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_pools() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        assert_eq!(factory.all_pools().len(), 0);

        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let tc = Address::generate(&env);

        factory.create_pool(&ta, &tb, &30_i128);
        assert_eq!(factory.all_pools().len(), 1);

        factory.create_pool(&ta, &tc, &30_i128);
        assert_eq!(factory.all_pools().len(), 2);
    }
}
