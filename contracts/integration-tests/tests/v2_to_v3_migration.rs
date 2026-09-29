//! Integration tests for migrating a V2 LP position into the real
//! `ConcentratedLiquidity` contract.

use amm::{AmmPool, AmmPoolClient};
use cl_position_nft::{ClPositionNft, ClPositionNftClient};
use concentrated_liquidity::{ConcentratedLiquidity, ConcentratedLiquidityClient};
use soroban_sdk::{
    testutils::Address as _,
    token::{StellarAssetClient, TokenClient},
    Address, Env, String,
};
use token::{LpToken, LpTokenClient};
use v2_to_v3_migration::{MigrationContract, MigrationContractClient, MigrationError};

const DEADLINE: u64 = u64::MAX;

struct Fixture<'a> {
    admin: Address,
    lp: Address,
    token_a: TokenClient<'a>,
    token_b: TokenClient<'a>,
    v2_pool: Address,
    v2_lp: LpTokenClient<'a>,
    v3_pool: Address,
    v3: ConcentratedLiquidityClient<'a>,
    migration_addr: Address,
    migration: MigrationContractClient<'a>,
    nft: Option<ClPositionNftClient<'a>>,
}

fn create_sac<'a>(env: &'a Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let contract = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &contract.address()),
        StellarAssetClient::new(env, &contract.address()),
    )
}

impl<'a> Fixture<'a> {
    fn setup(
        env: &'a Env,
        reversed: bool,
        tick_spacing: i32,
        initial_tick: i32,
        with_nft: bool,
        lp_deposit_a: i128,
        lp_deposit_b: i128,
    ) -> Self {
        env.budget().reset_unlimited();
        env.mock_all_auths();

        let admin = Address::generate(env);
        let lp = Address::generate(env);
        let fee_recipient = Address::generate(env);
        let (token_a, token_a_sac) = create_sac(env, &admin);
        let (token_b, token_b_sac) = create_sac(env, &admin);

        let v2_pool = env.register_contract(None, AmmPool);
        let v2_lp_addr = env.register_contract(None, LpToken);
        let v2_lp = LpTokenClient::new(env, &v2_lp_addr);
        v2_lp.initialize(
            &v2_pool,
            &String::from_str(env, "V2 LP"),
            &String::from_str(env, "V2LP"),
            &7,
        );
        let v2 = AmmPoolClient::new(env, &v2_pool);
        v2.initialize(
            &admin,
            &token_a.address,
            &token_b.address,
            &v2_lp_addr,
            &30,
            &fee_recipient,
            &0,
        );

        token_a_sac.mint(&admin, &10_000_000);
        token_b_sac.mint(&admin, &10_000_000);
        v2.add_liquidity(&admin, &6_000_000, &6_000_000, &0, &DEADLINE);

        token_a_sac.mint(&lp, &lp_deposit_a);
        token_b_sac.mint(&lp, &lp_deposit_b);
        v2.add_liquidity(&lp, &lp_deposit_a, &lp_deposit_b, &0, &DEADLINE);

        let v3_pool = env.register_contract(None, ConcentratedLiquidity);
        let v3 = ConcentratedLiquidityClient::new(env, &v3_pool);
        let (v3_token_a, v3_token_b) = if reversed {
            (token_b.address.clone(), token_a.address.clone())
        } else {
            (token_a.address.clone(), token_b.address.clone())
        };
        v3.initialize(
            &admin,
            &v3_token_a,
            &v3_token_b,
            &30,
            &initial_tick,
            &tick_spacing,
        );

        let nft = if with_nft {
            let nft_addr = env.register_contract(None, ClPositionNft);
            let nft = ClPositionNftClient::new(env, &nft_addr);
            nft.initialize(&admin, &v3_pool);
            v3.set_position_nft(&admin, &Some(nft_addr));
            Some(nft)
        } else {
            None
        };

        let migration_addr = env.register_contract(None, MigrationContract);
        let migration = MigrationContractClient::new(env, &migration_addr);
        migration.initialize(&admin, &v2_pool, &v3_pool);

        Self {
            admin,
            lp,
            token_a,
            token_b,
            v2_pool,
            v2_lp,
            v3_pool,
            v3,
            migration_addr,
            migration,
            nft,
        }
    }

    fn standard(env: &'a Env) -> Self {
        Self::setup(env, false, 60, 17, true, 1_000_000, 1_000_000)
    }
}

#[test]
fn migrate_mints_real_cl_position_to_lp_and_reports_nft() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    let shares = f.v2_lp.balance(&f.lp);

    let before_a = f.token_a.balance(&f.lp);
    let before_b = f.token_b.balance(&f.lp);
    let result = f.migration.migrate(
        &f.lp,
        &shares,
        &0,
        &0,
        &i32::MIN,
        &i32::MAX,
        &100,
        &0,
        &0,
        &DEADLINE,
    );

    assert_eq!((result.tick_lower, result.tick_upper), (-120, 120));
    assert!(result.deposited_a > 0);
    assert!(result.deposited_b > 0);
    assert_eq!(f.v2_lp.balance(&f.lp), 0);
    assert_eq!(f.token_a.balance(&f.lp) - before_a, result.leftover_a);
    assert_eq!(f.token_b.balance(&f.lp) - before_b, result.leftover_b);
    assert_eq!(f.token_a.balance(&f.v3_pool), result.deposited_a);
    assert_eq!(f.token_b.balance(&f.v3_pool), result.deposited_b);
    assert_eq!(f.token_a.balance(&f.migration_addr), 0);
    assert_eq!(f.token_b.balance(&f.migration_addr), 0);

    let position = f.v3.get_position(&f.lp, &-120, &120);
    assert!(position.liquidity > 0);
    let token_id = result.position_token_id.expect("position NFT expected");
    assert_eq!(f.nft.as_ref().unwrap().owner_of(&token_id), f.lp);
}

#[test]
fn migrate_without_nft_reports_none() {
    let env = Env::default();
    let f = Fixture::setup(&env, false, 10, 0, false, 1_000_000, 1_000_000);
    let shares = f.v2_lp.balance(&f.lp);

    let result = f
        .migration
        .migrate(&f.lp, &shares, &0, &0, &-100, &100, &0, &0, &0, &DEADLINE);

    assert_eq!(result.position_token_id, None);
    assert!(f.v3.get_position(&f.lp, &-100, &100).liquidity > 0);
}

#[test]
fn reversed_v3_pair_maps_amounts_and_leftovers_to_v3_order() {
    let env = Env::default();
    let f = Fixture::setup(&env, true, 10, 0, false, 1_000_000, 2_000_000);
    let shares = f.v2_lp.balance(&f.lp);
    let before_a = f.token_a.balance(&f.lp);
    let before_b = f.token_b.balance(&f.lp);

    let result = f
        .migration
        .migrate(&f.lp, &shares, &0, &0, &-100, &100, &0, &0, &0, &DEADLINE);

    // V3 token_a is the V2 pool's token_b, and V3 token_b is V2 token_a.
    assert_eq!(f.token_b.balance(&f.v3_pool), result.deposited_a);
    assert_eq!(f.token_a.balance(&f.v3_pool), result.deposited_b);
    assert_eq!(f.token_b.balance(&f.lp) - before_b, result.leftover_a);
    assert_eq!(f.token_a.balance(&f.lp) - before_a, result.leftover_b);
    assert!(f.v3.get_position(&f.lp, &-100, &100).liquidity > 0);
}

#[test]
fn auto_range_aligns_and_clamps_to_usable_cl_ticks() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    assert_eq!(
        f.migration.preview_range(&i32::MIN, &i32::MAX, &100),
        (-120, 120)
    );

    let near_max_env = Env::default();
    let near_max = Fixture::setup(
        &near_max_env,
        false,
        60,
        887_250,
        false,
        1_000_000,
        1_000_000,
    );
    assert_eq!(
        near_max
            .migration
            .preview_range(&i32::MIN, &i32::MAX, &1_000),
        (886_200, 887_220)
    );
}

#[test]
fn explicit_misaligned_range_is_rejected_before_v2_withdrawal() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    let shares_before = f.v2_lp.balance(&f.lp);
    let info_before = AmmPoolClient::new(&env, &f.v2_pool).get_info();

    let result = f.migration.try_migrate(
        &f.lp,
        &shares_before,
        &0,
        &0,
        &-119,
        &120,
        &0,
        &0,
        &0,
        &DEADLINE,
    );

    assert!(matches!(result, Err(Ok(MigrationError::InvalidRange))));
    assert_eq!(f.v2_lp.balance(&f.lp), shares_before);
    let info_after = AmmPoolClient::new(&env, &f.v2_pool).get_info();
    assert_eq!(info_after.reserve_a, info_before.reserve_a);
    assert_eq!(info_after.reserve_b, info_before.reserve_b);
    assert!(f.v3.get_positions(&f.lp).is_empty());
}

#[test]
fn per_token_v3_minimum_reverts_the_entire_migration() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    let shares_before = f.v2_lp.balance(&f.lp);
    let before_a = f.token_a.balance(&f.lp);
    let before_b = f.token_b.balance(&f.lp);

    let result = f.migration.try_migrate(
        &f.lp,
        &shares_before,
        &0,
        &0,
        &-120,
        &120,
        &0,
        &i128::MAX,
        &0,
        &DEADLINE,
    );

    assert!(result.is_err());
    assert_eq!(f.v2_lp.balance(&f.lp), shares_before);
    assert_eq!(f.token_a.balance(&f.lp), before_a);
    assert_eq!(f.token_b.balance(&f.lp), before_b);
    assert!(f.v3.get_positions(&f.lp).is_empty());
}

#[test]
fn initialize_rejects_a_different_v3_pair() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    let (other, _) = create_sac(&env, &f.admin);
    let bad_pool = env.register_contract(None, ConcentratedLiquidity);
    ConcentratedLiquidityClient::new(&env, &bad_pool).initialize(
        &f.admin,
        &f.token_a.address,
        &other.address,
        &30,
        &0,
        &10,
    );
    let migration_addr = env.register_contract(None, MigrationContract);

    let result = MigrationContractClient::new(&env, &migration_addr)
        .try_initialize(&f.admin, &f.v2_pool, &bad_pool);

    assert!(matches!(result, Err(Ok(MigrationError::TokenMismatch))));
}

#[test]
fn preview_and_migrate_use_the_same_final_range() {
    let env = Env::default();
    let f = Fixture::standard(&env);
    let expected = f.migration.preview_range(&i32::MIN, &i32::MAX, &100);
    let shares = f.v2_lp.balance(&f.lp);

    let result = f.migration.migrate(
        &f.lp,
        &shares,
        &0,
        &0,
        &i32::MIN,
        &i32::MAX,
        &100,
        &0,
        &0,
        &DEADLINE,
    );

    assert_eq!((result.tick_lower, result.tick_upper), expected);
}
