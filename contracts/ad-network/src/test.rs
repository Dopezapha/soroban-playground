#![cfg(test)]
use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env, String};

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
    AdNetworkContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let stored: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
    assert_eq!(stored, admin);
}

#[test]
fn test_create_campaign() {
    let (env, admin, token) = setup();
    AdNetworkContract::initialize(env.clone(), admin, token).unwrap();
    let id = AdNetworkContract::create_campaign(
        env.clone(),
        Address::generate(&env),
        String::from_str(&env, "Test Ad"),
        100_000,
        10,
        0,
        86400,
    )
    .unwrap();
    assert_eq!(id, 1);
}

#[test]
fn test_register_publisher() {
    let (env, admin, token) = setup();
    AdNetworkContract::initialize(env.clone(), admin.clone(), token).unwrap();
    let id = AdNetworkContract::register_publisher(
        env.clone(),
        admin,
        Address::generate(&env),
        String::from_str(&env, "example.com"),
    )
    .unwrap();
    assert_eq!(id, 1);
}

#[test]
fn test_record_and_verify_impression() {
    let (env, admin, token) = setup();
    AdNetworkContract::initialize(env.clone(), admin.clone(), token).unwrap();

    let adv = Address::generate(&env);
    let campaign_id = AdNetworkContract::create_campaign(
        env.clone(),
        adv,
        String::from_str(&env, "Campaign"),
        100_000,
        10,
        0,
        86400,
    )
    .unwrap();

    let pub_id = AdNetworkContract::register_publisher(
        env.clone(),
        admin.clone(),
        Address::generate(&env),
        String::from_str(&env, "site.com"),
    )
    .unwrap();

    let imp_id = AdNetworkContract::record_impression(
        env.clone(),
        campaign_id,
        pub_id,
        String::from_str(&env, "user123"),
    )
    .unwrap();

    AdNetworkContract::verify_impression(env.clone(), admin, imp_id).unwrap();

    let imp = AdNetworkContract::get_impression(env, imp_id).unwrap();
    assert!(imp.verified);
}
