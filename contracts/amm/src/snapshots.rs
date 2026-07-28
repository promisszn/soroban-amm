use soroban_sdk::{contracttype, Env, Vec};

#[contracttype]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub ledger: u32,
    pub reserve_a: i128,
    pub reserve_b: i128,
    pub total_shares: i128,
    pub accrued_fee_a: i128,
    pub accrued_fee_b: i128,
    pub price_range_low: i128,
    pub price_range_high: i128,
}

pub const MAX_SNAPSHOTS: usize = 1000; // simple cap

pub fn snapshot_position(env: &Env) {
    let ledger = env.ledger().sequence();
    let reserve_a = super::AmmPool::get_reserve_a(env.clone());
    let reserve_b = super::AmmPool::get_reserve_b(env.clone());
    let total_shares = super::AmmPool::get_total_shares(env.clone());
    let (accrued_a, accrued_b) = super::AmmPool::get_accrued_fees_internal(env.clone());

    let price = if reserve_a > 0 { reserve_b * 1_000_000 / reserve_a } else { 0 };
    let low = price * 9 / 10;
    let high = price * 11 / 10;
    let snap = Snapshot {
        ledger,
        reserve_a,
        reserve_b,
        total_shares,
        accrued_fee_a: accrued_a,
        accrued_fee_b: accrued_b,
        price_range_low: low,
        price_range_high: high,
    };

    let mut snaps: Vec<Snapshot> = get_snapshots(env);
    if snaps.len() >= MAX_SNAPSHOTS as u32 {
        snaps.remove(0);
    }
    snaps.push_back(snap);
    env.storage().instance().set(&super::DataKey::Snapshots, &snaps);
}

pub fn get_snapshots(env: &Env) -> Vec<Snapshot> {
    env.storage()
        .instance()
        .get(&super::DataKey::Snapshots)
        .unwrap_or_else(|| Vec::new(env))
}
