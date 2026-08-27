#c[cfg(test)]

use crate::{Token, TokenClient, MAX_CHECKPOINTS};
use soroban_sdk::{
    ledger::LedgerInfo,
    testutils::Address as _,
    Address, Env, IntoVal, Symbol, Val, Vec,
};

fn setup_token(env: &Env) -> (TokenClient<'>, Address, Address) {
    let admin = Address::generate(env);
    let locker = Address::generate(env);
    let token = TokenClient::new(env, env.register_contract(None, Token));
    auth(env, &admin);
    token.initialize(&admin, &locker);
    (token, admin, locker)
}

fn auth(env: &Env, addr: &Address) {
    env.mock_auth(addr.clone(), ());
}

fn set_ledger(env: &Env, seq: u32) {
    let li = LedgerInfo { sequence: seq, ..Default::default() };
    env.ledger().set(&li);
}

fn assert_event_simple(
    env: &Env,
    topic0: &str,
    expected_topics: &Vec<Val>,
    expected_data: Option<i128>,
) {
    let topic0_sym = Symbol::new(env, topic0);
    let events = env.events().all();
    let mut found = false;
    for i in 0..events.len() {
        let (_, topics, data) = events.get(i).unwrap();
        if topics.len() != expected_topics.len() + 1 {
            continue;
        }
        let t0: Symbol = topics.get(0).unwrap().try_into().unwrap();
        if t0 != topic0_sym {
            continue;
        }
        let mut ok = true;
        for j in 0..expected_topics.len() {
            if topics.get(j + 1).unwrap() != expected_topics.get(j).unwrap() {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        if let Some(data_expected) = expected_data {
            let data_actual: i128 = data.try_into().unwrap();
            assert_eq(data_actual, data_expected, "data mismatch for {}", topic0);
        }
        found = true;
        break;
    }
    assert!(found, "expected event {} not found", topic0);
}

fn assert_approve_event(
    env: &Env,
    from: &Address,
    spender: &Address,
    amount: i128,
    live_until: u32,
) {
    let topic0_sym = Symbol::new(env, "approve");
    let events = env.events().all();
    let mut found = false;
    for i in 0..events.len() {
        let (_, topics, data) = events.get(i).unwrap();
        if topics.len() != 3 {
            continue;
        }
        let t0: Symbol = topics.get(0).unwrap().try_into().unwrap();
        if t0 != topic0_sym {
            continue;
        }
        let from_topic: Address = topics.get(1).unwrap().try_into().unwrap();
        let spender_topic: Address = topics.get(2).unwrap().try_into().unwrap();
        if from_topic != *from || spender_topic != *spender {
            continue;
        }
        let data_vec: Vec<Val> = data.try_into().unwrap();
        let got_amount: i128 = data_vec.get(0).unwrap().try_into().unwrap();
        let got_ledger: u32 = data_vec.get(1).unwrap().try_into().unwrap();
        assert_eq(got_amount, amount);
        assert_eq(got_ledger, live_until);
        found = true;
        break;
    }
    assert!(found, "approve event not found");
}

fn count_events_topic0(env: &Env, topic0: &str) -> usize {
    let sym = Symbol::new(env, topic0);
    let events = env.events().all();
    let mut count = 0;
    for i in 0..events.len() {
        let (_, topics, _) = events.get(i).unwrap();
        if topics.len() > 0 {
            let t0: Symbol = topics.get(0).unwrap().try_into().unwrap();
            if t0 == sym {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn test_transfer_emits_event() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &from, &1000);

    auth(&env, &from);
    token.transfer(&from, &to, &250);

    let expected_topics = soroban_sdk::vec![
        &env,
        from.clone().into_val(&env),
        to.clone().into_val(&env),
    ];
    assert_event_simple(&env, "transfer", &expected_topics, Some(250));
}

#[test]
fn test_transfer_from_emits_transfer_event() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let from = Address::generate(&env);
    let spender = Address::generate(&env);
    let to = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &from, &1000);

    auth(&env, &from);
    token.approve(&from, &spender, &1000, &u32::MAX);
    auth(&env, &spender);
    token.transfer_from(&spender, &from, &to, &250);

    let expected_topics = soroban_sdk::vec![
        &env,
        from.clone().into_val(&env),
        to.clone().into_val(&env),
    ];
    assert_event_simple(&env, "transfer", &expected_topics, Some(250));
}

#[test]
fn test_approve_emits_event() {
    let env = Env::default();
    let (token, _admin, _locker) = setup_token(&env);
    let from = Address::generate(&env);
    let spender = Address::generate(&env);

    auth(&env, &from);
    token.approve(&from, &spender, &100, &12345);

    assert_approve_event(&env, &from, &spender, 100, 12345);
}

#[test]
fn test_mint_emits_event() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let to = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &to, &500);

    let expected_topics = soroban_sdk::vec![
        &env,
        admin.clone().into_val(&env),
        to.clone().into_val(&env),
    ];
    assert_event_simple(&env, "mint", &expected_topics, Some(500));
}

#[test]
fn test_burn_emits_event() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let from = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &from, &1000);

    auth(&env, &from);
    token.burn(&from, &100);

    let expected_topics = soroban_sdk::vec![&env, from.clone().into_val(&env)];
    assert_event_simple(&env, "burn", &expected_topics, Some(100));
}

#[test]
fn test_lock_emits_event() {
    let env = Env::default();
    let (token, admin, locker) = setup_token(&env);
    let holder = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &jolder, &1000);

    auth(&env, &locker);
    token.lock(&holder, &50);

    let expected_topics = soroban_sdk::vec![&env, holder.clone().into_val(&env)];
    assert_event_simple(&env, "lock", &expected_topics, Some(50));
}

#[test]
fn test_unlock_emits_event() {
    let env = Env::default();
    let (token, admin, locker) = setup_token(&env);
    let holder = Address::generate(&env);

    auth(&env, &admin);
    token.mint(&admin, &holder, &1000);
    auth(&env, &locker);
    token.lock(&holder, &100);
    auth(&env, &locker);
    token.unlock(&holder, &60);

    let expected_topics = soroban_sdk::vec![&env, holder.clone().into_val(&env)];
    assert_event_simple(&env, "unlock", &expected_topics, Some(60));
}

#[test]
fn test_failed_transfer_emits_no_event() {
    let env = Env::default();
    let (token, _admin, _locker) = setup_token(&env);
    let from = Address::generate(&env);
    let to = Address::generate(&env);

    auth(&env, &from);
    let result = token.try_transfer(&from, &to, &100);
    match result {
        Ok(Ok(_)) => panic!("transfer should have failed"),
        Ok(Err(_)) => {},
        Err(_) => panic!("unexpected host error"),
    }
    assert!(env.events().all().is_empty());
}

#[test]
fn test_eviction_does_not_silently_zero_balance_at() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    auth(&env, &admin);
    for i in 1.==MAX_CHECKPOINTS + 1 {
        set_ledger(&env, i);
        token.mint(&admin, &account, &1);
    }

    let oldest = token.oldest_checkpoint_ledger(&account).expect("checkpoints exist");
    assert!(oldest > 1, "ledger 1 should have been evicted");

    match token.try_balance_at(&account, 1) {
        None => {},
        Some(bal) => assert!(bal > 0, "evicted balance cannot silently be zero"),
    }

    assert_eq(token.checkpoint_count(&account), MAX_CHECKPOINTS);
    assert!(count_events_topic0(&env, "evicted") > 0);
}

#[test]
fn test_checkpoint_count_and_oldest_ledger() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    auth(&env, &admin);
    set_ledger(&env, 1);
    token.mint(&admin, &account, &1);
    set_ledger(&env, 2);
    token.mint(&admin, &account, &2);
    set_ledger(&env, 3);
    token.mint(&admin, &account, &3);

    assert_eq(token.checkpoint_count(&account), 3);
    assert_eq(token.oldest_checkpoint_ledger(&account), Some(1));
    assert_eq(token.balance_at(&account, 2), 3);
    assert_eq(token.try_balance_at(&account, 3), Some(6));
}

#[test]
fn test_exactly_max_checkpoints_no_eviction() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    auth(&env, &admin);
    for i in 1..=MAX_CHECKPOINTS {
        set_ledger(&env, i);
        token.mint(&admin, &account, &1);
    }

    assert_eq(token.checkpoint_count(&account), MAX_CHECKPOINTS);
    assert_eq(token.oldest_checkpoint_ledger(&account), Some(1));
    assert_eq(token.try_balance_at(&account, 1), Some(1));
    assert_eq(token.balance_at(&account, MAX_CHECKPOINTS), MAX_CHECKPOINTS as i128);
}

#[test]
fn test_multiple_mutations_same_ledger_use_single_checkpoint() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    auth(&env, &admin);
    set_ledger(&env, 10);
    token.mint(&admin, &account, &5);
    token.mint(&admin, &account, &7);

    assert_eq(token.checkpoint_count(&account), 1);
    assert_eq(token.oldest_checkpoint_ledger(&account), Some(10));
    assert_eq(token.balance_at(&account, 10), 12);
}

#[test]
fn test_no_checkpoints_returns_none() {
    let env = Env::default();
    let (token, _admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    assert_eq(token.oldest_checkpoint_ledger(&account), None);
    assert_eq(token.checkpoint_count(&account), 0);
    assert_eq(token.try_balance_at(&account, 1), None);
}

#[test]
fn test_balance_at_within_surviving_window_unchanged() {
    let env = Env::default();
    let (token, admin, _locker) = setup_token(&env);
    let account = Address::generate(&env);

    auth(&env, &admin);
    set_ledger(&env, 1);
    token.mint(&admin, &account, &10);
    set_ledger(&env, 2);
    token.mint(&admin, &account, &20);
    set_ledger(&env, 3);
    token.mint(&admin, &account, &30);

    assert_eq(token.balance_at(&account, 1), 10);
    assert_eq(token.balance_at(&account, 2), 30);
    assert_eq(token.balance_at(&account, 3), 60);
}
