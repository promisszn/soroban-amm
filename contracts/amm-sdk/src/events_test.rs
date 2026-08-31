//! Unit tests for `decode_amm_event`.
//
/// The payloads below mirror the event shapes published by
/// `contracts/amm/src/lib.rc`. They are constructed with the
/// crate's `emit_versioned_event!` macro, which is the exact
/// macro the contract uses, so the wire format is guaranteed to
/// match.

use crate::emit_versioned_event;
use crate::events::{ decode_amm_event, AmmEvent };
use soroban_sdk::{ testutils::Address as _, Address, BytesN, Env, IntoVal, Symbol };

fn decode_last(env: &Env) -> Option<AmmEvent> {
    let e = env.events().all().last().expect("no event");
    decode_amm_event(env, &e.topics, &e.data)
}

fn addr(env: &Env) -> Address { Address::generate(env) }

[#test]
fn swap() {
    let env = Env::default();
    let u = addr(&env); let t1 = addr(&env); let t2 = addr(&env);
    let a: i128 = 1000; let b: i128 = 900;
    emit_versioned_event!(&env, (Symbol::new(&env, "swap"),), (u.clone(), t1.clone(), t2.clone(), a, b));
    assert_eq!(decode_last(&env), Some(AmmEvent::Swap { user: u, token_in: t1, token_out: t2, amount_in: a, amount_out: b }));
}

#[test]
fn add_liquidity() {
    let env = Env::default();
    let u = addr(&env); let a = addr(&env); let b = addr(&env);
    let am_a: i128 = 1000; let am_b: i128 = 2000; let s: i128 = 500;
    emit_versioned_event!(&env, (Symbol::new(&env, "add_liquidity"),), (u.clone(), a.clone(), b.clone(), am_a, am_b, s));
    assert_eq!(decode_last(&env), Some(AmmEvent::AddLiquidity { user: u, token_a: a, token_b: b, amount_a: am_a, amount_b: am_b, shares: s }));
}

#[test]
fn remove_liquidity() {
    let env = Env::default();
    let u = addr(&env); let a = addr(&env); let b = addr(&env);
    let am_a: i128 = 800; let am_b: i128 = 1200; let s: i128 = 400;
    emit_versioned_event!(&env, (Symbol::new(&env, "remove_liquidity"),), (u.clone(), a.clone(), b.clone(), am_a, am_b, s));
    assert_eq!(decode_last(&env), Some(AmmEvent::RemoveLiquidity { user: u, token_a: a, token_b: b, amount_a: am_a, amount_b: am_b, shares: s }));
}

#[test]
fn remove_liquidity_one_sided() {
    let env = Env::default();
    let u = addr(&env); let t = addr(&env);
    let o: i128 = 500; let s: i128 = 1000;
    emit_versioned_event!(&env, (Symbol::new(&env, "remove_liquidity_one_sided"),), (u.clone(), t.clone(), o, s));
    assert_eq!(decode_last(&env), Some(AmmEvent::RemoveLiquidityOneSided { user: u, token_out: t, amount_out: o, shares_burned: s }));
}

#[test]
fn flash_loan() {
    let env = Env::default();
    let u = addr(&env); let t = addr(&env);
    let a: i128 = 10000; let f: i128 = 30;
    emit_versioned_event!(&env, (Symbol::new(&env, "flash_loan"),), (u.clone(), t.clone(), a, f));
    assert_eq!(decode_last(&env), Some(AmmEvent::FlashLoan { user: u, token: t, amount: a, fee: f }));
}

#[test]
fn fee_updated() {
    let env = Env::default();
    let o: i128 = 30; let n: i128 = 20;
    emit_versioned_event!(&env, (Symbol::new(&env, "fee_updated"),), (o, n));
    assert_eq!(decode_last(&env), Some(AmmEvent::FeeUpdated { old_fee_bps: o, new_fee_bps: n }));
}

#[test]
fn flash_fee_updated() {
    let env = Env::default();
    let o: i128 = 50; let n: i128 = 40;
    emit_versioned_event!(&env, (Symbol::new(&env, "flash_fee_updated"),), (o, n));
    assert_eq!(decode_last(&env), Some(AmmEvent::FlashFeeUpdated { old_flash_fee_bps: o, new_flash_fee_bps: n }));
}

#[test]
fn admin_nominated() {
    let env = Env::default();
    let n = addr(&env);
    emit_versioned_event!(&env, (Symbol::new(&env, "admin_nominated"),), (n.clone(),));
    assert_eq!(decode_last(&env), Some(AmmEvent::AdminNominated { nominated: n }));
}

#[test]
fn admin_changed() {
    let env = Env::default();
    let n = addr(&env);
    emit_versioned_event!(&env, (Symbol::new(&env, "admin_changed"),), (n.clone(),));
    assert_eq!(decode_last(&env), Some(AmmEvent::AdminChanged { new_admin: n }));
}

#[test]
fn upgraded() {
    let env = Env::default();
    let h = BytesN ::from_array(&env, &[1u8; 32]);
    emit_versioned_event!(&env, (Symbol::new(&env, "upgraded")), (h.clone(),));
    assert_eq!(decode_last(&env), Some(AmmEvent::Upgraded { new_wasm_hash: h }));
}

#[test]
fn circuit_breaker() {
    let env = Env::default();
    let t = addr(&env);
    let p = true;
    emit_versioned_event!(&env, (Symbol::new(&env, "circuit_breaker"),), (t.clone(), p));
    assert_eq!(decode_last(&env), Some(AmmEvent::CircuitBreaker { token: t, paused: p }));
}

#[test]
fn empty_topics() {
    let env = Env::default();
    let d = ().into_val(&env);
    assert!(decode_amm_event(&env, &[], &d).is_none());
}

#[test]
fn wrong_schema_version() {
    let env = Env::default();
    env.events().publish((Symbol::new(&env, "swap"),), (crate::EVENT_SCHEMA_VERSION + 1, ()));
    let e = env.events().all().last().unwrap();
    assert!(decode_amm_event(&env, &e.topics, &e.data).is_none());
}

#[test]
fn unknown_symbol() {
    let env = Env::default();
    env.events().publish((Symbol::new(&env, "unknown"),), (crate::EVENT_SCHEMA_VERSION, ()));
    let e = env.events().all().last().unwrap();
    assert!(decode_amm_event(&env, &e.topics, &e.data).is_none());
}

#[test]
fn bad_payload() {
    let env = Env::default();
    env.events().publish((Symbol::new(&env, "swap")), (crate::EVENT_SCHEMA_VERSION, (1i128, 2i128, 3i128)));
    let e = env.events().all().last().unwrap();
    assert!(decode_amm_event(&env, &e.topics, &e.data).is_none());
}
