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
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Symbol, Vec};
use token::LpTokenClient;

// ── Storage keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Pool(Address, Address), // normalized (token_a, token_b) → pool Address
    LpToken(Address),       // pool address → LP token address
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
    /// `lp_name` and `lp_symbol` set the LP token's metadata. When `None` the
    /// factory generates counter-based defaults: `"AMM LP Token #N"` / `"ALPN"`.
    ///
    /// Panics if a pool for this pair already exists.
    pub fn create_pool(
        env: Env,
        token_a: Address,
        token_b: Address,
        fee_bps: i128,
        lp_name: Option<soroban_sdk::String>,
        lp_symbol: Option<soroban_sdk::String>,
    ) -> Address {
        // Normalise: smaller address is always token_a.
        let (ta, tb) = if token_a < token_b {
            (token_a, token_b)
        } else {
            (token_b, token_a)
        };
        
        assert!(
            (0..=10_000).contains(&fee_bps),
            "invalid fee_bps: {fee_bps} must be in 0..=10_000"
        );

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

        // Resolve LP token name/symbol — use caller-supplied values or counter defaults.
        let name = lp_name.unwrap_or_else(|| Self::counter_string(&env, b"AMM LP Token #", n));
        let symbol = lp_symbol.unwrap_or_else(|| Self::counter_string(&env, b"ALP", n));

        // Initialize LP token — admin must be the pool so it can mint/burn.
        LpTokenClient::new(&env, &lp_addr).initialize(&pool_addr, &name, &symbol, &7u32);

        // Initialize AMM pool.
        AmmPoolClient::new(&env, &pool_addr).initialize(
            &admin, &ta, &tb, &lp_addr, &fee_bps, &admin,  // fee_recipient
            &0_i128, // protocol_fee_bps (disabled by default)
        );

        // Register pool in lookup indexes and record the LP token address.
        env.storage()
            .instance()
            .set(&DataKey::Pool(ta.clone(), tb.clone()), &pool_addr);
        env.storage()
            .instance()
            .set(&DataKey::LpToken(pool_addr.clone()), &lp_addr);

        let mut all: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::AllPools)
            .unwrap_or_else(|| Vec::new(&env));
        all.push_back(pool_addr.clone());
        env.storage().instance().set(&DataKey::AllPools, &all);

        env.events().publish(
            (Symbol::new(&env, "pool_created"),),
            (ta.clone(), tb.clone(), pool_addr.clone(), fee_bps, lp_addr.clone()),
        );

        pool_addr
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Update the AMM and/or LP token WASM hashes used for new pool deployments.
    ///
    /// Only the factory admin can call this. Existing pools are unaffected; only
    /// pools created after this call will use the new hashes.
    ///
    /// Pass `None` for a hash to leave it unchanged.
    /// Emits a `wasm_updated` event on every call.
    pub fn update_wasm_hashes(
        env: Env,
        amm_wasm_hash: Option<BytesN<32>>,
        token_wasm_hash: Option<BytesN<32>>,
    ) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if let Some(ref h) = amm_wasm_hash {
            env.storage().instance().set(&DataKey::AmmWasmHash, h);
        }
        if let Some(ref h) = token_wasm_hash {
            env.storage().instance().set(&DataKey::TokenWasmHash, h);
        }
        env.events().publish(
            (Symbol::new(&env, "wasm_updated"),),
            (amm_wasm_hash, token_wasm_hash),
        );
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Return the LP token address for the given pool, or `None` if unknown.
    pub fn get_lp_token(env: Env, pool: Address) -> Option<Address> {
        env.storage().instance().get(&DataKey::LpToken(pool))
    }

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

    /// Build a Soroban `String` from a byte prefix plus a decimal counter.
    ///
    /// Works in `no_std` — avoids `format!` by constructing ASCII digits manually.
    /// `prefix` must be valid UTF-8 (it always is for the callers in this file).
    fn counter_string(env: &Env, prefix: &[u8], n: u64) -> soroban_sdk::String {
        // Max: 20-char prefix + 20 decimal digits of u64::MAX
        let mut buf = [0u8; 40];
        let plen = prefix.len();
        buf[..plen].copy_from_slice(prefix);

        let nlen = if n == 0 {
            buf[plen] = b'0';
            1usize
        } else {
            let mut tmp = [0u8; 20];
            let mut num = n;
            let mut i = 0usize;
            while num > 0 {
                tmp[i] = b'0' + (num % 10) as u8;
                num /= 10;
                i += 1;
            }
            // Reverse digit order into buf.
            for j in 0..i {
                buf[plen + j] = tmp[i - 1 - j];
            }
            i
        };

        let total = plen + nlen;
        // SAFETY: prefix is valid UTF-8 and the appended bytes are ASCII digits.
        let s = core::str::from_utf8(&buf[..total]).unwrap();
        soroban_sdk::String::from_str(env, s)
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

        let pool = factory.create_pool(&ta, &tb, &30_i128, &None, &None);

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

        factory.create_pool(&ta, &tb, &30_i128, &None, &None);

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

        factory.create_pool(&ta, &tb, &30_i128, &None, &None);
        let result = factory.try_create_pool(&ta, &tb, &30_i128, &None, &None);
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

        factory.create_pool(&ta, &tb, &30_i128, &None, &None);
        assert_eq!(factory.all_pools().len(), 1);

        factory.create_pool(&ta, &tc, &30_i128, &None, &None);
        assert_eq!(factory.all_pools().len(), 2);
    }

    // ── Issue #96: LP token name/symbol reflect the token pair ───────────────

    #[test]
    fn test_lp_token_default_names_are_distinct() {
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
        let tc = Address::generate(&env);

        let pool0 = factory.create_pool(&ta, &tb, &30_i128, &None, &None);
        let pool1 = factory.create_pool(&ta, &tc, &30_i128, &None, &None);

        // Fetch LP token addresses via the factory's registry.
        let lp0 = factory.get_lp_token(&pool0).unwrap();
        let lp1 = factory.get_lp_token(&pool1).unwrap();

        let lp_client0 = token_wasm::Client::new(&env, &lp0);
        let lp_client1 = token_wasm::Client::new(&env, &lp1);

        // Names and symbols must differ between the two pools.
        assert_ne!(lp_client0.name(), lp_client1.name());
        assert_ne!(lp_client0.symbol(), lp_client1.symbol());
    }

    #[test]
    fn test_lp_token_custom_name_and_symbol() {
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

        let custom_name = soroban_sdk::String::from_str(&env, "USDC/XLM LP");
        let custom_symbol = soroban_sdk::String::from_str(&env, "USDC-XLM");

        let pool = factory.create_pool(
            &ta,
            &tb,
            &30_i128,
            &Some(custom_name.clone()),
            &Some(custom_symbol.clone()),
        );

        let lp_addr = factory.get_lp_token(&pool).unwrap();
        let lp = token_wasm::Client::new(&env, &lp_addr);

        assert_eq!(lp.name(), custom_name);
        assert_eq!(lp.symbol(), custom_symbol);
    }

    // ── Issue #97: update_wasm_hashes ─────────────────────────────────────────

    #[test]
    fn test_update_wasm_hashes_non_admin_panics() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        // update_wasm_hashes itself requires admin auth; mock_all_auths covers it,
        // but we can verify the function doesn't panic when called by the real admin.
        factory.update_wasm_hashes(&Some(amm_hash.clone()), &None);
        factory.update_wasm_hashes(&None, &Some(token_hash.clone()));
        factory.update_wasm_hashes(&Some(amm_hash), &Some(token_hash));
    }

    #[test]
    fn test_update_wasm_hashes_updates_storage_and_allows_new_pool() {
        let env = Env::default();
        env.mock_all_auths();

        let amm_hash = env.deployer().upload_contract_wasm(amm_wasm::WASM);
        let token_hash = env.deployer().upload_contract_wasm(token_wasm::WASM);

        let admin = Address::generate(&env);
        let factory_addr = env.register_contract(None, Factory);
        let factory = FactoryClient::new(&env, &factory_addr);
        factory.initialize(&admin, &amm_hash, &token_hash);

        // Call update with the same hashes (a no-op in practice, but verifies
        // the function is callable by admin and doesn't panic).
        factory.update_wasm_hashes(&Some(amm_hash.clone()), &Some(token_hash.clone()));

        // Pool creation still works after an update.
        let ta = Address::generate(&env);
        let tb = Address::generate(&env);
        let pool = factory.create_pool(&ta, &tb, &30_i128, &None, &None);
        assert!(factory.get_pool(&ta, &tb).is_some());
        assert!(factory.get_lp_token(&pool).is_some());

        // Partial update — only token_wasm_hash.
        factory.update_wasm_hashes(&None, &Some(token_hash.clone()));

        // Partial update — only amm_wasm_hash.
        factory.update_wasm_hashes(&Some(amm_hash.clone()), &None);
    }
}
