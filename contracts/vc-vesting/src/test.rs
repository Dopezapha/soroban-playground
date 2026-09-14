#![cfg(test)]
use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    (env, admin, token)
}

#[test]
fn test_initialize() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token.clone()).unwrap();
    let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
    assert_eq!(stored, admin);
}

#[test]
fn test_initialize_twice_fails() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token.clone()).unwrap();
    assert_eq!(
        VCVestingContract::initialize(env.clone(), admin, token),
        Err(VCVestingError::AlreadyInitialized)
    );
}

#[test]
fn test_create_pool() {
    let (env, admin, _token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), Address::generate(&env)).unwrap();
    let pool_id = VCVestingContract::create_pool(
        env.clone(),
        String::from_str(&env, "VC Fund I"),
        1_000_000,
        3,
    )
    .unwrap();
    assert_eq!(pool_id, 1);
    let pool = VCVestingContract::get_pool(env.clone(), pool_id).unwrap();
    assert_eq!(pool.name, String::from_str(&env, "VC Fund I"));
    assert_eq!(pool.tranche_count, 3);
}

#[test]
fn test_add_tranche() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let pool_id =
        VCVestingContract::create_pool(env.clone(), String::from_str(&env, "Fund"), 1_000_000, 3)
            .unwrap();
    VCVestingContract::add_tranche(
        env.clone(),
        pool_id,
        1,
        300_000,
        String::from_str(&env, "MVP Launch"),
        6000,
    )
    .unwrap();
    let t = VCVestingContract::get_tranche(env, pool_id, 1).unwrap();
    assert_eq!(t.total_amount, 300_000);
    assert_eq!(t.required_votes_bps, 6000);
}

#[test]
fn test_add_investor() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let pool_id =
        VCVestingContract::create_pool(env.clone(), String::from_str(&env, "Fund"), 1_000_000, 2)
            .unwrap();
    let inv = Address::generate(&env);
    VCVestingContract::add_investor(env.clone(), pool_id, inv.clone(), 500_000).unwrap();
    let stored = VCVestingContract::get_investor(env, pool_id, inv).unwrap();
    assert_eq!(stored.allocation, 500_000);
}

#[test]
fn test_open_voting() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let pool_id =
        VCVestingContract::create_pool(env.clone(), String::from_str(&env, "Fund"), 1_000_000, 1)
            .unwrap();
    VCVestingContract::add_tranche(
        env.clone(),
        pool_id,
        1,
        500_000,
        String::from_str(&env, "Milestone"),
        5100,
    )
    .unwrap();
    VCVestingContract::open_voting(env.clone(), pool_id, 1, 86400).unwrap();
    let t = VCVestingContract::get_tranche(env, pool_id, 1).unwrap();
    assert!(t.voting_end_time > 0);
}

#[test]
fn test_vote_and_finalize() {
    let (env, admin, token) = setup();
    VCVestingContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let pool_id =
        VCVestingContract::create_pool(env.clone(), String::from_str(&env, "Fund"), 1_000_000, 1)
            .unwrap();
    VCVestingContract::add_tranche(
        env.clone(),
        pool_id,
        1,
        500_000,
        String::from_str(&env, "Milestone"),
        5100,
    )
    .unwrap();

    let inv1 = Address::generate(&env);
    let inv2 = Address::generate(&env);
    VCVestingContract::add_investor(env.clone(), pool_id, inv1.clone(), 400_000).unwrap();
    VCVestingContract::add_investor(env.clone(), pool_id, inv2.clone(), 600_000).unwrap();

    VCVestingContract::open_voting(env.clone(), pool_id, 1, 86400).unwrap();

    VCVestingContract::vote(env.clone(), pool_id, 1, inv1.clone(), true).unwrap();
    VCVestingContract::vote(env.clone(), pool_id, 1, inv2.clone(), true).unwrap();

    assert!(VCVestingContract::has_voted(
        env.clone(),
        pool_id,
        1,
        inv1.clone()
    ));

    env.ledger().set_timestamp(env.ledger().timestamp() + 86401);

    let approved = VCVestingContract::finalize_voting(env.clone(), pool_id, 1).unwrap();
    assert!(approved);

    let claimable = VCVestingContract::claim_tokens(env.clone(), pool_id, 1, inv1).unwrap();
    assert!(claimable > 0);
}
