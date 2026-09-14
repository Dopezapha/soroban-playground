// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Env};

const INITIAL_PRICE: i128 = 100_000_000;
const SIZE: i128 = 1_000_000;
const COLLATERAL: i128 = 200_000;

fn setup() -> (
    Env,
    PerpetualFuturesClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, PerpetualFutures);
    let client = PerpetualFuturesClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let trader = Address::generate(&env);
    let liquidator = Address::generate(&env);
    client.initialize(&admin, &INITIAL_PRICE);
    (env, client, admin, trader, liquidator)
}

#[test]
fn test_initialize_sets_admin() {
    let (_env, client, admin, ..) = setup();
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_initialize_twice_fails() {
    let (_env, client, admin, ..) = setup();
    assert!(client.try_initialize(&admin, &INITIAL_PRICE).is_err());
}

#[test]
fn test_open_long_position() {
    let (_env, client, _admin, trader, _) = setup();
    let id = client.open_position(&trader, &true, &SIZE, &10u32, &COLLATERAL);
    assert_eq!(id, 1);
    assert_eq!(client.position_count(), 1);

    let pos = client.get_position(&id);
    assert_eq!(pos.trader, trader);
    assert!(pos.is_long);
    assert_eq!(pos.size, SIZE);
    assert_eq!(pos.leverage, 10);
    assert_eq!(pos.status, PositionStatus::Active);
}

#[test]
fn test_open_short_position() {
    let (_env, client, _admin, trader, _) = setup();
    let id = client.open_position(&trader, &false, &SIZE, &5u32, &COLLATERAL);
    let pos = client.get_position(&id);
    assert!(!pos.is_long);
    assert_eq!(pos.leverage, 5);
}

#[test]
fn test_invalid_leverage_fails() {
    let (_env, client, _admin, trader, _) = setup();
    assert!(client
        .try_open_position(&trader, &true, &SIZE, &0u32, &COLLATERAL)
        .is_err());
    assert!(client
        .try_open_position(&trader, &true, &SIZE, &101u32, &COLLATERAL)
        .is_err());
}

#[test]
fn test_close_position_long() {
    let (_env, client, _admin, trader, _) = setup();
    let id = client.open_position(&trader, &true, &SIZE, &10u32, &COLLATERAL);

    let exit_price = INITIAL_PRICE + 10_000_000;
    let net = client.close_position(&trader, &id, &exit_price);
    assert!(net > COLLATERAL);

    let pos = client.get_position(&id);
    assert_eq!(pos.status, PositionStatus::Closed);
}

#[test]
fn test_close_position_short() {
    let (_env, client, _admin, trader, _) = setup();
    let id = client.open_position(&trader, &false, &SIZE, &10u32, &COLLATERAL);

    let exit_price = INITIAL_PRICE - 10_000_000;
    let net = client.close_position(&trader, &id, &exit_price);
    assert!(net > COLLATERAL);
}

#[test]
fn test_update_funding_rate() {
    let (_env, client, admin, ..) = setup();
    let mark = INITIAL_PRICE + 1_000_000;
    let index = INITIAL_PRICE;

    let bps = client.update_funding_rate(&admin, &mark, &index);
    assert_eq!(bps, 100);

    let funding = client.get_funding_rate();
    assert_eq!(funding.mark_price, mark);
    assert_eq!(funding.rate_bps, 100);
}

#[test]
fn test_liquidate_position() {
    let (_env, client, _admin, trader, liquidator) = setup();
    let id = client.open_position(&trader, &true, &SIZE, &50u32, &COLLATERAL);

    let crash_price = INITIAL_PRICE / 2;
    client.liquidate_position(&liquidator, &id, &crash_price);

    let pos = client.get_position(&id);
    assert_eq!(pos.status, PositionStatus::Liquidated);
}

#[test]
fn test_pause_blocks_operations() {
    let (_env, client, admin, trader, _) = setup();
    client.set_paused(&admin, &true);
    assert!(client
        .try_open_position(&trader, &true, &SIZE, &10u32, &COLLATERAL)
        .is_err());
}
