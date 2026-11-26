//! Paged tick-bitmap word index (#1311): cost independence from the number of
//! active words, and the bounded migration of pools upgraded from the legacy
//! sorted `TickBitmapWords` vector.

extern crate std;

use crate::{
    ClError, ConcentratedLiquidity, ConcentratedLiquidityClient, DataKey, MIN_BITMAP_WORD_POS,
};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env, Vec};

/// Pages needed to cover every bitmap word (`-6932..=6931`).
const PAGE_COUNT: i32 = 109;

struct TestEnv {
    env: Env,
    token_a: Address,
    token_b: Address,
    pool_addr: Address,
    client: ConcentratedLiquidityClient<'static>,
}

fn setup_test() -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let admin = Address::generate(&env);
    let token_a = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_b = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    let pool_addr = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool_addr);
    client.initialize(&admin, &token_a, &token_b, &30_i128, &0_i32, &1_i32);

    TestEnv {
        env,
        token_a,
        token_b,
        pool_addr,
        client,
    }
}

fn funded(te: &TestEnv) -> Address {
    let who = Address::generate(&te.env);
    StellarAssetClient::new(&te.env, &te.token_a).mint(&who, &1_000_000_000_000);
    StellarAssetClient::new(&te.env, &te.token_b).mint(&who, &1_000_000_000_000);
    who
}

fn page_of(word_pos: i32) -> (i32, u32) {
    let offset = word_pos - MIN_BITMAP_WORD_POS;
    (offset / 128, (offset % 128) as u32)
}

/// Every index page plus the root, as stored.
fn paged_index(te: &TestEnv) -> (std::vec::Vec<u128>, Option<u128>) {
    te.env.as_contract(&te.pool_addr, || {
        let storage = te.env.storage().persistent();
        let pages = (0..PAGE_COUNT)
            .map(|p| {
                storage
                    .get::<_, u128>(&DataKey::TickBitmapIndexPage(p))
                    .unwrap_or(0)
            })
            .collect();
        let root = te
            .env
            .storage()
            .instance()
            .get(&DataKey::TickBitmapIndexRoot);
        (pages, root)
    })
}

/// Rewrites a pool's paged index back into the pre-#1311 layout: a sorted
/// `TickBitmapWords` vector, no pages, no root, no migrated flag. This is the
/// storage a pool initialized before the paged index has after upgrading.
fn convert_to_legacy_index(te: &TestEnv) {
    let (pages, _) = paged_index(te);
    te.env.as_contract(&te.pool_addr, || {
        let mut words = Vec::<i32>::new(&te.env);
        for (p, bits) in pages.iter().enumerate() {
            for bit in 0..128 {
                if bits & (1_u128 << bit) != 0 {
                    words.push_back(MIN_BITMAP_WORD_POS + p as i32 * 128 + bit);
                }
            }
            te.env
                .storage()
                .persistent()
                .remove(&DataKey::TickBitmapIndexPage(p as i32));
        }
        let storage = te.env.storage();
        storage.instance().remove(&DataKey::TickBitmapIndexRoot);
        storage.instance().remove(&DataKey::TickBitmapIndexMigrated);
        storage.persistent().set(&DataKey::TickBitmapWords, &words);
    });
}

fn legacy_words(te: &TestEnv) -> Option<std::vec::Vec<i32>> {
    te.env.as_contract(&te.pool_addr, || {
        te.env
            .storage()
            .persistent()
            .get::<_, Vec<i32>>(&DataKey::TickBitmapWords)
            .map(|w| w.iter().collect())
    })
}

fn is_migrated(te: &TestEnv) -> bool {
    te.env.as_contract(&te.pool_addr, || {
        te.env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::TickBitmapIndexMigrated)
            .unwrap_or(false)
    })
}

fn has_cursor(te: &TestEnv) -> bool {
    te.env.as_contract(&te.pool_addr, || {
        te.env
            .storage()
            .instance()
            .has(&DataKey::TickBitmapIndexCursor)
    })
}

/// Lookups from a spread of query ticks in both directions.
fn lookups(te: &TestEnv, queries: &[i32]) -> std::vec::Vec<(Option<i32>, Option<i32>)> {
    queries
        .iter()
        .map(|q| {
            (
                te.client.next_initialized_tick_pub(q),
                te.client.prev_initialized_tick_pub(q),
            )
        })
        .collect()
}

const QUERIES: [i32; 14] = [
    -887_000, -50_001, -40_000, -20_001, -4_500, -301, -1, 0, 299, 4_500, 20_000, 39_999, 50_000,
    887_000,
];

/// Mints the same set of positions, spread over several index pages, in a pool.
fn mint_ladder(te: &TestEnv, lp: &Address) {
    let ranges = [
        (-40_000, -20_000),
        (-20_000, -300),
        (-300, 300),
        (300, 20_000),
        (20_000, 40_000),
    ];
    for (lower, upper) in ranges {
        te.client.mint_position(
            lp,
            &lower,
            &upper,
            &1_000_000_000,
            &1_000_000_000,
            &0,
            &0,
            &u64::MAX,
        );
    }
}

/// Applies the same trade to both pools and checks they agree on output,
/// resulting tick and active liquidity.
fn swap_both(control: &TestEnv, legacy: &TestEnv, zero_for_one: bool, amount: i128) -> i32 {
    let t1 = funded(control);
    let t2 = funded(legacy);
    let out1 = control
        .client
        .swap(&t1, &zero_for_one, &amount, &0, &0, &u64::MAX);
    let out2 = legacy
        .client
        .swap(&t2, &zero_for_one, &amount, &0, &0, &u64::MAX);
    assert_eq!(out1, out2);
    let tick = control.client.current_tick();
    assert_eq!(tick, legacy.client.current_tick());
    assert_eq!(
        control.client.active_liquidity(),
        legacy.client.active_liquidity()
    );
    tick
}

#[test]
fn test_fresh_pool_starts_migrated_with_no_root() {
    let te = setup_test();
    assert!(is_migrated(&te));
    assert_eq!(legacy_words(&te), None);
    assert_eq!(te.client.migrate_tick_bitmap_index(&100), 0);

    // First mint on a fresh pool creates the root from "absent == empty"
    // without consulting any other page.
    let lp = funded(&te);
    te.client
        .mint_position(&lp, &-300, &300, &1_000_000, &1_000_000, &0, &0, &u64::MAX);
    let (pages, root) = paged_index(&te);
    let (p, _) = page_of(-300_i32.div_euclid(128));
    assert_eq!(root, Some(1_u128 << p));
    assert_eq!(pages.iter().filter(|b| **b != 0).count(), 1);
    assert_eq!(te.client.next_initialized_tick_pub(&-1_000), Some(-300));

    // Closing the last position empties every page and removes the root;
    // the next mint rebuilds both from scratch.
    let liq = te.client.get_position(&lp, &-300, &300).liquidity;
    te.client.burn_position(&lp, &-300, &300, &liq);
    let (pages, root) = paged_index(&te);
    assert_eq!(root, None);
    assert!(pages.iter().all(|b| *b == 0));
    assert_eq!(te.client.next_initialized_tick_pub(&-1_000), None);

    te.client.mint_position(
        &lp,
        &-20_000,
        &300,
        &1_000_000,
        &1_000_000,
        &0,
        &0,
        &u64::MAX,
    );
    assert_eq!(te.client.next_initialized_tick_pub(&-30_000), Some(-20_000));
    assert_eq!(te.client.prev_initialized_tick_pub(&0), Some(-20_000));
}

#[test]
fn test_migrate_requires_initialized_pool() {
    let env = Env::default();
    let pool_addr = env.register_contract(None, ConcentratedLiquidity);
    let client = ConcentratedLiquidityClient::new(&env, &pool_addr);
    assert_eq!(
        client.try_migrate_tick_bitmap_index(&10),
        Err(Ok(ClError::NotInitialized))
    );
}

#[test]
fn test_legacy_pool_migrates_in_batches_and_swaps_cross_same_ticks() {
    let control = setup_test();
    let legacy = setup_test();
    let lp_c = funded(&control);
    let lp_l = funded(&legacy);
    mint_ladder(&control, &lp_c);
    mint_ladder(&legacy, &lp_l);

    convert_to_legacy_index(&legacy);
    assert!(!is_migrated(&legacy));
    let words_before = legacy_words(&legacy).unwrap();
    assert_eq!(words_before.len(), 6);

    // Before migration the legacy vector still answers every lookup.
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));

    // Trading and minting keep working (and keep the vector current) while
    // the pool has not been migrated.
    let tick = swap_both(&control, &legacy, true, 2_000_000_000);
    assert!(tick < -300, "swap should cross tick -300, ended at {tick}");
    for (te, lp) in [(&control, &lp_c), (&legacy, &lp_l)] {
        te.client.mint_position(
            lp,
            &-4_500,
            &-4_400,
            &1_000_000,
            &1_000_000,
            &0,
            &0,
            &u64::MAX,
        );
    }
    // -4_500 and -4_400 sit in two different words.
    assert_eq!(legacy_words(&legacy).unwrap().len(), 8);
    assert!(!has_cursor(&legacy));
    assert_eq!(
        paged_index(&legacy),
        (std::vec![0; PAGE_COUNT as usize], None)
    );

    // Migrate two words per batch. Legacy words, ascending:
    // -313, -157, -36, -35, -3, 2, 156, 312.
    let mint_both = |lower: i32, upper: i32| {
        for (te, lp) in [(&control, &lp_c), (&legacy, &lp_l)] {
            te.client.mint_position(
                lp,
                &lower,
                &upper,
                &1_000_000,
                &1_000_000,
                &0,
                &0,
                &u64::MAX,
            );
        }
    };
    let burn_both = |lower: i32, upper: i32| {
        for (te, lp) in [(&control, &lp_c), (&legacy, &lp_l)] {
            let liq = te.client.get_position(lp, &lower, &upper).liquidity;
            te.client.burn_position(lp, &lower, &upper, &liq);
        }
    };

    // Batch 1 migrates -313, -157; cursor -36.
    assert_eq!(legacy.client.migrate_tick_bitmap_index(&2), 6);
    assert!(has_cursor(&legacy));
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));
    // Words -305/-304 open behind the cursor: mirrored into the pages.
    mint_both(-39_000, -38_900);
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));

    // Batch 2 migrates -36, -35; cursor -3.
    assert_eq!(legacy.client.migrate_tick_bitmap_index(&2), 4);
    // Words 234/235 open ahead of the cursor: left for a later batch.
    mint_both(30_000, 30_100);
    // Words -36/-35 close behind the cursor: cleared from the pages too.
    burn_both(-4_500, -4_400);
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));

    // Batch 3 migrates -3, 2; cursor 156 (remaining 156, 234, 235, 312).
    assert_eq!(legacy.client.migrate_tick_bitmap_index(&2), 4);
    // Words 234/235 close again while still ahead of the cursor.
    burn_both(30_000, 30_100);
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));

    // Batch 4 migrates 156, 312 and finishes.
    assert_eq!(legacy.client.migrate_tick_bitmap_index(&2), 0);

    // Migration finished: the paged index is identical to one built
    // incrementally, and the legacy state is gone.
    assert!(is_migrated(&legacy));
    assert!(!has_cursor(&legacy));
    assert_eq!(legacy_words(&legacy), None);
    assert_eq!(paged_index(&legacy), paged_index(&control));
    assert_eq!(legacy.client.migrate_tick_bitmap_index(&2), 0);
    assert_eq!(lookups(&legacy, &QUERIES), lookups(&control, &QUERIES));

    // Swaps after migration cross the same ticks in both directions.
    let tick = swap_both(&control, &legacy, false, 6_000_000_000);
    assert!(tick > 300, "swap should cross tick 300, ended at {tick}");
    let tick = swap_both(&control, &legacy, true, 9_000_000_000);
    assert!(
        tick < -20_000,
        "swap should cross tick -20000, ended at {tick}"
    );
}

#[test]
fn test_migration_batch_writes_at_most_sixteen_pages() {
    let te = setup_test();
    // One active word on each of the first 20 pages, in the legacy layout.
    let words: std::vec::Vec<i32> = (0..20).map(|p| MIN_BITMAP_WORD_POS + p * 128 + 5).collect();
    te.env.as_contract(&te.pool_addr, || {
        let mut legacy = Vec::<i32>::new(&te.env);
        for w in &words {
            legacy.push_back(*w);
            te.env
                .storage()
                .persistent()
                .set(&DataKey::TickBitmap(*w), &1_u128);
        }
        te.env
            .storage()
            .persistent()
            .set(&DataKey::TickBitmapWords, &legacy);
        te.env
            .storage()
            .instance()
            .remove(&DataKey::TickBitmapIndexMigrated);
    });

    let first_tick = words[0] * 128;
    let last_tick = words[19] * 128;
    assert_eq!(
        te.client.next_initialized_tick_pub(&-887_272),
        Some(first_tick)
    );
    assert_eq!(
        te.client.prev_initialized_tick_pub(&887_272),
        Some(last_tick)
    );

    // max_words would allow everything, but the batch stops at 16 pages.
    assert_eq!(te.client.migrate_tick_bitmap_index(&1_000), 4);
    let (pages, root) = paged_index(&te);
    assert_eq!(pages.iter().filter(|b| **b != 0).count(), 16);
    assert_eq!(root, Some((1_u128 << 16) - 1));
    assert!(!is_migrated(&te));

    assert_eq!(te.client.migrate_tick_bitmap_index(&1_000), 0);
    let (pages, root) = paged_index(&te);
    assert!(is_migrated(&te));
    assert_eq!(root, Some((1_u128 << 20) - 1));
    for (p, bits) in pages.iter().enumerate() {
        assert_eq!(*bits, if p < 20 { 1_u128 << 5 } else { 0 }, "page {p}");
    }
    for pair in words.windows(2) {
        assert_eq!(
            te.client.next_initialized_tick_pub(&(pair[0] * 128)),
            Some(pair[1] * 128)
        );
        assert_eq!(
            te.client.prev_initialized_tick_pub(&(pair[1] * 128 - 1)),
            Some(pair[0] * 128)
        );
    }
    assert_eq!(
        te.client.next_initialized_tick_pub(&-887_272),
        Some(first_tick)
    );
    assert_eq!(
        te.client.prev_initialized_tick_pub(&887_272),
        Some(last_tick)
    );
}

/// Indexes `words` as active bitmap words, either in the paged index or
/// (`legacy`) in the pre-#1311 sorted vector, then measures the CPU and memory
/// cost of one mint that opens word 5 (on a populated index page, so both
/// runs take the same page-update path). Only the index is seeded: the mint
/// never reads the other words' bitmaps. The test host's cost model charges
/// every call in proportion to the total number of ledger entries (see the
/// `cl_position_nft` holdings cost tests, which need a 60x ceiling because
/// they do not control for it), so the pool is padded with unrelated entries
/// up to `INDEX_ENTRY_BUDGET` index entries in every run.
fn mint_cost_with_indexed_words(words: &[i32], legacy: bool) -> (u64, u64) {
    const INDEX_ENTRY_BUDGET: u64 = PAGE_COUNT as u64;
    let te = setup_test();
    let provider = funded(&te);

    te.env.budget().reset_unlimited();
    te.env.as_contract(&te.pool_addr, || {
        let storage = te.env.storage().persistent();
        let mut index_entries = 0_u64;
        if legacy {
            let mut legacy_words = Vec::<i32>::new(&te.env);
            for w in words {
                legacy_words.push_back(*w);
            }
            storage.set(&DataKey::TickBitmapWords, &legacy_words);
            index_entries += 1;
            te.env
                .storage()
                .instance()
                .remove(&DataKey::TickBitmapIndexMigrated);
        } else {
            let mut pages = [0_u128; PAGE_COUNT as usize];
            for w in words {
                let (page, bit) = page_of(*w);
                pages[page as usize] |= 1_u128 << bit;
            }
            let mut root = 0_u128;
            for (page, bits) in pages.iter().enumerate() {
                if *bits != 0 {
                    storage.set(&DataKey::TickBitmapIndexPage(page as i32), bits);
                    root |= 1_u128 << page;
                    index_entries += 1;
                }
            }
            te.env
                .storage()
                .instance()
                .set(&DataKey::TickBitmapIndexRoot, &root);
        }
        for i in index_entries..INDEX_ENTRY_BUDGET {
            storage.set(&DataKey::OraclePoint(i), &0_i64);
        }
    });

    te.env.budget().reset_default();
    te.client
        .mint_position(&provider, &700, &710, &100_000, &0, &0, &0, &u64::MAX);
    (
        te.env.budget().cpu_instruction_cost(),
        te.env.budget().memory_bytes_cost(),
    )
}

/// Mint cost must not depend on how many bitmap words are active (#1311).
///
/// Compares a mint that opens a new word with 5 other active words against
/// one with every other word in the tick domain active (13,863). The paged
/// index touches one page entry either way: measured ratios are ~1.01 (CPU)
/// and ~1.00 (memory). `COST_RATIO_CEILING` leaves 10% headroom for
/// incidental cost and is checked to be meaningful: the same measurement
/// against the legacy sorted vector, which reads and rewrites one `i32` per
/// active word on every word-opening mint, measures ~1.33 (CPU) and ~6.6
/// (memory) and must exceed it on both.
#[test]
fn test_mint_cost_independent_of_active_bitmap_word_count() {
    const COST_RATIO_CEILING: f64 = 1.1;
    let small: std::vec::Vec<i32> = (6..11).collect();
    let large: std::vec::Vec<i32> = (MIN_BITMAP_WORD_POS..=crate::MAX_BITMAP_WORD_POS)
        .filter(|w| *w != 5)
        .collect();

    let ratios = |legacy: bool| {
        let (small_cpu, small_mem) = mint_cost_with_indexed_words(&small, legacy);
        let (large_cpu, large_mem) = mint_cost_with_indexed_words(&large, legacy);
        std::println!(
            "legacy={legacy}: cpu {small_cpu} -> {large_cpu}, mem {small_mem} -> {large_mem}"
        );
        (
            large_cpu as f64 / small_cpu as f64,
            large_mem as f64 / small_mem as f64,
        )
    };

    let (cpu, mem) = ratios(false);
    assert!(
        cpu < COST_RATIO_CEILING && mem < COST_RATIO_CEILING,
        "expected mint cost to be independent of active bitmap word count, got cpu ratio {cpu}, mem ratio {mem}"
    );
    let (legacy_cpu, legacy_mem) = ratios(true);
    assert!(
        legacy_cpu > COST_RATIO_CEILING && legacy_mem > COST_RATIO_CEILING,
        "ceiling {COST_RATIO_CEILING} no longer separates the paged index from the legacy sorted vector (cpu ratio {legacy_cpu}, mem ratio {legacy_mem})"
    );
}
