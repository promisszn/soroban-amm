//! Concentrated Liquidity AMM (Uniswap v3-style tick-based ranges).
//! Standalone contract — does NOT modify the existing AMM pool.
#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env};
use soroban_sdk::token::Client as TokenClient;

const PRICE_SCALE: i128 = 1_000_000;
const TICK_BASE_NUM: i128 = 1_000_100;
const TICK_BASE_DEN: i128 = PRICE_SCALE;
const MIN_TICK: i32 = -887_272;
const MAX_TICK: i32 = 887_272;

#[contracttype]
pub enum DataKey {
    TokenA, TokenB, FeeBps, CurrentTick,
    FeeGrowthGlobalA, FeeGrowthGlobalB, ActiveLiquidity,
    Position(Address, i32, i32),
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: i128,
    pub fee_growth_inside_a: i128,
    pub fee_growth_inside_b: i128,
    pub tokens_owed: (i128, i128),
}

#[contract]
pub struct ConcentratedLiquidity;

#[contractimpl]
impl ConcentratedLiquidity {
    /// One-time initialisation. Sets token pair, fee, and starting tick.
    pub fn initialize(env: Env, token_a: Address, token_b: Address, fee_bps: i128, initial_tick: i32) {
        assert!(!env.storage().instance().has(&DataKey::TokenA), "already initialized");
        assert!(token_a != token_b, "tokens must differ");
        assert!((0..=10_000).contains(&fee_bps), "invalid fee_bps");
        assert!(initial_tick >= MIN_TICK && initial_tick <= MAX_TICK, "tick out of range");
        env.storage().instance().set(&DataKey::TokenA, &token_a);
        env.storage().instance().set(&DataKey::TokenB, &token_b);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::CurrentTick, &initial_tick);
        env.storage().instance().set(&DataKey::FeeGrowthGlobalA, &0_i128);
        env.storage().instance().set(&DataKey::FeeGrowthGlobalB, &0_i128);
        env.storage().instance().set(&DataKey::ActiveLiquidity, &0_i128);
    }    pub fn mint_position(env: Env, provider: Address, lower_tick: i32, upper_tick: i32, amount_a_desired: i128, amount_b_desired: i128, min_a: i128, min_b: i128) -> (i128, i128) {
        provider.require_auth();
        assert!(lower_tick < upper_tick, "lower_tick must be < upper_tick");
        assert!(lower_tick >= MIN_TICK && upper_tick <= MAX_TICK, "tick out of range");
        assert!(amount_a_desired > 0 || amount_b_desired > 0, "zero amounts");
        let current_tick: i32 = env.storage().instance().get(&DataKey::CurrentTick).unwrap();
        let token_a: Address = env.storage().instance().get(&DataKey::TokenA).unwrap();
        let token_b: Address = env.storage().instance().get(&DataKey::TokenB).unwrap();
        let (amount_a, amount_b) = Self::amounts_for_liquidity(current_tick, lower_tick, upper_tick, amount_a_desired, amount_b_desired);
        assert!(amount_a >= min_a, "slippage: amount_a too low");
        assert!(amount_b >= min_b, "slippage: amount_b too low");
        let liquidity = Self::liquidity_from_amounts(current_tick, lower_tick, upper_tick, amount_a, amount_b);
        assert!(liquidity > 0, "liquidity would be zero");
        if amount_a > 0 { TokenClient::new(&env, &token_a).transfer(&provider, &env.current_contract_address(), &amount_a); }
        if amount_b > 0 { TokenClient::new(&env, &token_b).transfer(&provider, &env.current_contract_address(), &amount_b); }
        let pos_key = DataKey::Position(provider.clone(), lower_tick, upper_tick);
        let fg_a: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalA).unwrap_or(0);
        let fg_b: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalB).unwrap_or(0);
        let mut pos: Position = env.storage().instance().get(&pos_key).unwrap_or(Position { lower_tick, upper_tick, liquidity: 0, fee_growth_inside_a: fg_a, fee_growth_inside_b: fg_b, tokens_owed: (0, 0) });
        let (oa, ob) = Self::pending_fees(&pos, fg_a, fg_b);
        pos.tokens_owed = (pos.tokens_owed.0 + oa, pos.tokens_owed.1 + ob);
        pos.fee_growth_inside_a = fg_a; pos.fee_growth_inside_b = fg_b;
        pos.liquidity += liquidity;
        env.storage().instance().set(&pos_key, &pos);
        if current_tick >= lower_tick && current_tick < upper_tick {
            let active: i128 = env.storage().instance().get(&DataKey::ActiveLiquidity).unwrap_or(0);
            env.storage().instance().set(&DataKey::ActiveLiquidity, &(active + liquidity));
        }
        env.events().publish((symbol_short!("mint_pos"), provider), (lower_tick, upper_tick, liquidity, amount_a, amount_b));
        (amount_a, amount_b)
    }
    pub fn burn_position(env: Env, provider: Address, lower_tick: i32, upper_tick: i32, liquidity: i128) -> (i128, i128) {
        provider.require_auth();
        assert!(liquidity > 0, "liquidity must be positive");
        let pos_key = DataKey::Position(provider.clone(), lower_tick, upper_tick);
        let mut pos: Position = env.storage().instance().get(&pos_key).expect("position not found");
        assert!(pos.liquidity >= liquidity, "insufficient liquidity");
        let current_tick: i32 = env.storage().instance().get(&DataKey::CurrentTick).unwrap();
        let token_a: Address = env.storage().instance().get(&DataKey::TokenA).unwrap();
        let token_b: Address = env.storage().instance().get(&DataKey::TokenB).unwrap();
        let fg_a: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalA).unwrap_or(0);
        let fg_b: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalB).unwrap_or(0);
        let (oa, ob) = Self::pending_fees(&pos, fg_a, fg_b);
        pos.tokens_owed = (pos.tokens_owed.0 + oa, pos.tokens_owed.1 + ob);
        pos.fee_growth_inside_a = fg_a; pos.fee_growth_inside_b = fg_b;
        let (amount_a, amount_b) = Self::amounts_for_liquidity(current_tick, lower_tick, upper_tick, liquidity, liquidity);
        pos.liquidity -= liquidity;
        env.storage().instance().set(&pos_key, &pos);
        if current_tick >= lower_tick && current_tick < upper_tick {
            let active: i128 = env.storage().instance().get(&DataKey::ActiveLiquidity).unwrap_or(0);
            env.storage().instance().set(&DataKey::ActiveLiquidity, &(if active > liquidity { active - liquidity } else { 0 }));
        }
        if amount_a > 0 { TokenClient::new(&env, &token_a).transfer(&env.current_contract_address(), &provider, &amount_a); }
        if amount_b > 0 { TokenClient::new(&env, &token_b).transfer(&env.current_contract_address(), &provider, &amount_b); }
        env.events().publish((symbol_short!("burn_pos"), provider), (lower_tick, upper_tick, liquidity, amount_a, amount_b));
        (amount_a, amount_b)
    }
    pub fn collect_fees(env: Env, provider: Address, lower_tick: i32, upper_tick: i32) -> (i128, i128) {
        provider.require_auth();
        let pos_key = DataKey::Position(provider.clone(), lower_tick, upper_tick);
        let mut pos: Position = env.storage().instance().get(&pos_key).expect("position not found");
        let fg_a: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalA).unwrap_or(0);
        let fg_b: i128 = env.storage().instance().get(&DataKey::FeeGrowthGlobalB).unwrap_or(0);
        let (na, nb) = Self::pending_fees(&pos, fg_a, fg_b);
        let total_a = pos.tokens_owed.0 + na; let total_b = pos.tokens_owed.1 + nb;
        pos.tokens_owed = (0, 0); pos.fee_growth_inside_a = fg_a; pos.fee_growth_inside_b = fg_b;
        env.storage().instance().set(&pos_key, &pos);
        let token_a: Address = env.storage().instance().get(&DataKey::TokenA).unwrap();
        let token_b: Address = env.storage().instance().get(&DataKey::TokenB).unwrap();
        if total_a > 0 { TokenClient::new(&env, &token_a).transfer(&env.current_contract_address(), &provider, &total_a); }
        if total_b > 0 { TokenClient::new(&env, &token_b).transfer(&env.current_contract_address(), &provider, &total_b); }
        (total_a, total_b)
    }
    pub fn get_position(env: Env, provider: Address, lower_tick: i32, upper_tick: i32) -> Position {
        env.storage().instance().get(&DataKey::Position(provider, lower_tick, upper_tick)).expect("position not found")
    }
    pub fn current_tick(env: Env) -> i32 { env.storage().instance().get(&DataKey::CurrentTick).unwrap() }
    pub fn active_liquidity(env: Env) -> i128 { env.storage().instance().get(&DataKey::ActiveLiquidity).unwrap_or(0) }
    pub fn tick_to_price(tick: i32) -> i128 {
        if tick == 0 { return 1_000_000; }
        let abs_tick = tick.unsigned_abs() as i128;
        let iters = abs_tick.min(300);
        let mut price = 1_000_000_i128;
        for _ in 0..iters { price = price * 1_000_100 / 1_000_000; }
        if tick < 0 { price = 1_000_000 * 1_000_000 / price; }
        price
    }
    fn amounts_for_liquidity(ct: i32, lt: i32, ut: i32, ad: i128, bd: i128) -> (i128, i128) {
        if ct < lt { (ad, 0) } else if ct >= ut { (0, bd) } else {
            let pl = Self::tick_to_price(lt); let pu = Self::tick_to_price(ut); let pc = Self::tick_to_price(ct);
            let range = pu - pl; if range == 0 { return (ad/2, bd/2); }
            let below = pc - pl;
            (ad * (range - below) / range, bd * below / range)
        }
    }
    fn liquidity_from_amounts(ct: i32, lt: i32, ut: i32, a: i128, b: i128) -> i128 {
        if ct < lt { a } else if ct >= ut { b } else { a.min(b).max(1) }
    }
    fn pending_fees(pos: &Position, fg_a: i128, fg_b: i128) -> (i128, i128) {
        let da = fg_a - pos.fee_growth_inside_a; let db = fg_b - pos.fee_growth_inside_b;
        let oa = if da > 0 { pos.liquidity * da / 1_000_000 } else { 0 };
        let ob = if db > 0 { pos.liquidity * db / 1_000_000 } else { 0 };
        (oa, ob)
    }
}
