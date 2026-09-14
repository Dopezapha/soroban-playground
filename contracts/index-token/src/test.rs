#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env, String};

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    (env, admin)
}

#[test]
fn test_initialize() {
    let (env, admin) = setup();
    IndexTokenContract::initialize(env.clone(), admin.clone()).unwrap();
    let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
    assert_eq!(stored, admin);
}

#[test]
fn test_create_pool() {
    let (env, admin) = setup();
    IndexTokenContract::initialize(env.clone(), admin).unwrap();
    let id =
        IndexTokenContract::create_pool(env.clone(), String::from_str(&env, "DeFi Index"), 500)
            .unwrap();
    assert_eq!(id, 1);
    let pool = IndexTokenContract::get_pool(env.clone(), id).unwrap();
    assert_eq!(pool.name, String::from_str(&env, "DeFi Index"));
}

#[test]
fn test_add_asset() {
    let (env, admin) = setup();
    IndexTokenContract::initialize(env.clone(), admin).unwrap();
    let id =
        IndexTokenContract::create_pool(env.clone(), String::from_str(&env, "Index"), 500).unwrap();
    let asset_id = IndexTokenContract::add_asset(
        env.clone(),
        id,
        Address::generate(&env),
        String::from_str(&env, "XLM"),
        5000,
    )
    .unwrap();
    assert_eq!(asset_id, 1);
}

#[test]
fn test_deposit_and_withdraw() {
    let (env, admin) = setup();
    IndexTokenContract::initialize(env.clone(), admin).unwrap();
    let id =
        IndexTokenContract::create_pool(env.clone(), String::from_str(&env, "Index"), 500).unwrap();
    let inv = Address::generate(&env);
    let shares = IndexTokenContract::deposit(env.clone(), id, inv.clone(), 1000).unwrap();
    assert_eq!(shares, 1000);
    let amount = IndexTokenContract::withdraw(env.clone(), id, inv, 500).unwrap();
    assert_eq!(amount, 500);
}
