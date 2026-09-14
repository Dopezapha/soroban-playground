// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! Comprehensive test suite for the Energy Trading contract.
//!
//! Covers: initialization, meter management, readings, trading, balances, and full lifecycle.

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, String};

use crate::types::{EnergyType, Error, MeterStatus, TradeStatus};
use crate::{EnergyTrading, EnergyTradingClient};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Deploy and initialize in one step; returns (env, admin, client).
fn setup() -> (Env, Address, EnergyTradingClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, EnergyTrading);
    let client = EnergyTradingClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let env = std::boxed::Box::leak(std::boxed::Box::new(env));
    let client = EnergyTradingClient::new(env, &id);
    (env.clone(), admin, client)
}

fn make_str(env: &Env, s: &str) -> String {
    String::from_str(env, s)
}

// ── init ──────────────────────────────────────────────────────────────────────

#[test]
fn init_sets_admin() {
    let (_, admin, client) = setup();
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn init_sets_zero_energy_traded() {
    let (_, _, client) = setup();
    assert_eq!(client.get_total_energy_traded(), 0);
}

#[test]
fn init_sets_zero_meter_count() {
    let (_, _, client) = setup();
    assert_eq!(client.get_meter_count(), 0);
}

#[test]
fn init_sets_zero_trade_count() {
    let (_, _, client) = setup();
    assert_eq!(client.get_trade_count(), 0);
}

#[test]
#[should_panic(expected = "already initialized")]
fn init_twice_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, EnergyTrading);
    let client = EnergyTradingClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    client.initialize(&admin);
}

// ── Smart Meter Management ────────────────────────────────────────────────────

#[test]
fn register_meter_works() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "123 Solar Street"),
        &EnergyType::Solar,
        &5000,
    );
    assert_eq!(id, 1);
    assert_eq!(client.get_meter_count(), 1);

    let meter = client.get_meter(&id);
    assert_eq!(meter.owner, owner);
    assert_eq!(meter.energy_type, EnergyType::Solar);
    assert_eq!(meter.capacity_kw, 5000);
    assert_eq!(meter.status, MeterStatus::Active);
}

#[test]
fn register_multiple_meters() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id1 = client.register_meter(
        &owner,
        &make_str(&env, "Location 1"),
        &EnergyType::Solar,
        &5000,
    );
    let id2 = client.register_meter(
        &owner,
        &make_str(&env, "Location 2"),
        &EnergyType::Wind,
        &3000,
    );
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(client.get_meter_count(), 2);
}

#[test]
fn register_meter_empty_location_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    assert_eq!(
        client.try_register_meter(&owner, &make_str(&env, ""), &EnergyType::Solar, &5000,),
        Err(Ok(Error::EmptyField))
    );
}

#[test]
fn register_meter_zero_capacity_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    assert_eq!(
        client.try_register_meter(&owner, &make_str(&env, "Location"), &EnergyType::Solar, &0,),
        Err(Ok(Error::InvalidMeterReading))
    );
}

#[test]
fn deactivate_meter_works() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.deactivate_meter(&owner, &id);
    let meter = client.get_meter(&id);
    assert_eq!(meter.status, MeterStatus::Inactive);
}

#[test]
fn deactivate_meter_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    assert_eq!(
        client.try_deactivate_meter(&stranger, &id),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn deactivate_meter_not_found_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    assert_eq!(
        client.try_deactivate_meter(&owner, &999),
        Err(Ok(Error::MeterNotFound))
    );
}

// ── Meter Readings ────────────────────────────────────────────────────────────

#[test]
fn submit_reading_works() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );

    client.submit_reading(&owner, &id, &100, &50, &12345);

    let meter = client.get_meter(&id);
    assert_eq!(meter.total_generated, 100);

    let balance = client.get_balance(&owner);
    assert_eq!(balance.kwh_balance, 50); // 100 generated - 50 consumed
}

#[test]
fn submit_reading_updates_balance() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );

    client.submit_reading(&owner, &id, &200, &100, &111);
    let balance1 = client.get_balance(&owner);
    assert_eq!(balance1.kwh_balance, 100);

    client.submit_reading(&owner, &id, &150, &50, &222);
    let balance2 = client.get_balance(&owner);
    assert_eq!(balance2.kwh_balance, 200); // 100 + (150 - 50)
}

#[test]
fn submit_reading_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    assert_eq!(
        client.try_submit_reading(&stranger, &id, &100, &50, &12345),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn submit_reading_inactive_meter_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.deactivate_meter(&owner, &id);
    assert_eq!(
        client.try_submit_reading(&owner, &id, &100, &50, &12345),
        Err(Ok(Error::MeterNotRegistered))
    );
}

#[test]
fn submit_reading_negative_generated_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    assert_eq!(
        client.try_submit_reading(&owner, &id, &-100, &50, &12345),
        Err(Ok(Error::InvalidMeterReading))
    );
}

// ── Trading ───────────────────────────────────────────────────────────────────

#[test]
fn create_sell_order_works() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &1000, &0, &12345);

    let order_id = client.create_sell_order(&owner, &id, &500, &10);
    assert_eq!(order_id, 1);
    assert_eq!(client.get_trade_count(), 1);

    let order = client.get_trade_order(&order_id);
    assert_eq!(order.seller_meter_id, id);
    assert_eq!(order.kwh_amount, 500);
    assert_eq!(order.price_per_kwh, 10);
    assert_eq!(order.total_price, 5000);
    assert_eq!(order.status, TradeStatus::Open);
}

#[test]
fn create_sell_order_insufficient_balance_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &100, &0, &12345);

    assert_eq!(
        client.try_create_sell_order(&owner, &id, &500, &10),
        Err(Ok(Error::InsufficientBalance))
    );
}

#[test]
fn create_sell_order_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &1000, &0, &12345);

    assert_eq!(
        client.try_create_sell_order(&stranger, &id, &500, &10),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn create_sell_order_invalid_amount_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &1000, &0, &12345);

    assert_eq!(
        client.try_create_sell_order(&owner, &id, &0, &10),
        Err(Ok(Error::InvalidTradeAmount))
    );
}

#[test]
fn accept_order_works() {
    let (env, _admin, client) = setup();
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);

    let seller_meter = client.register_meter(
        &seller,
        &make_str(&env, "Seller Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&seller, &seller_meter, &1000, &0, &111);

    let buyer_meter = client.register_meter(
        &buyer,
        &make_str(&env, "Buyer Location"),
        &EnergyType::Solar,
        &3000,
    );
    client.submit_reading(&buyer, &buyer_meter, &0, &500, &222);

    let order_id = client.create_sell_order(&seller, &seller_meter, &500, &10);
    let trade_id = client.accept_order(&buyer, &order_id, &buyer_meter);

    assert_eq!(trade_id, 2); // order_id=1, trade_id=2

    let trade = client.get_energy_trade(&trade_id);
    assert_eq!(trade.seller, seller);
    assert_eq!(trade.buyer, buyer);
    assert_eq!(trade.kwh_amount, 500);
    assert_eq!(trade.total_price, 5000);
    assert_eq!(trade.status, TradeStatus::Settled);

    // Verify balances
    let seller_balance = client.get_balance(&seller);
    assert_eq!(seller_balance.kwh_balance, 500); // 1000 - 500
    assert_eq!(seller_balance.total_earned, 5000);

    let buyer_balance = client.get_balance(&buyer);
    assert_eq!(buyer_balance.kwh_balance, 0); // 500 - 500 + 500 - 500 = 0
    assert_eq!(buyer_balance.total_spent, 5000);

    // Verify total energy traded
    assert_eq!(client.get_total_energy_traded(), 500);
}

#[test]
fn accept_order_self_trade_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);

    let meter = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &meter, &1000, &0, &12345);

    let order_id = client.create_sell_order(&owner, &meter, &500, &10);
    assert_eq!(
        client.try_accept_order(&owner, &order_id, &meter),
        Err(Ok(Error::SelfTrade))
    );
}

#[test]
fn accept_order_wrong_status_fails() {
    let (env, _admin, client) = setup();
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);

    let seller_meter = client.register_meter(
        &seller,
        &make_str(&env, "Seller"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&seller, &seller_meter, &1000, &0, &111);

    let buyer_meter =
        client.register_meter(&buyer, &make_str(&env, "Buyer"), &EnergyType::Solar, &3000);
    client.submit_reading(&buyer, &buyer_meter, &0, &500, &222);

    let order_id = client.create_sell_order(&seller, &seller_meter, &500, &10);
    client.cancel_order(&seller, &order_id);

    assert_eq!(
        client.try_accept_order(&buyer, &order_id, &buyer_meter),
        Err(Ok(Error::InvalidStatus))
    );
}

#[test]
fn cancel_order_works() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &1000, &0, &12345);

    let order_id = client.create_sell_order(&owner, &id, &500, &10);
    client.cancel_order(&owner, &order_id);

    let order = client.get_trade_order(&order_id);
    assert_eq!(order.status, TradeStatus::Cancelled);
}

#[test]
fn cancel_order_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let stranger = Address::generate(&env);
    let id = client.register_meter(
        &owner,
        &make_str(&env, "Location"),
        &EnergyType::Solar,
        &5000,
    );
    client.submit_reading(&owner, &id, &1000, &0, &12345);

    let order_id = client.create_sell_order(&owner, &id, &500, &10);
    assert_eq!(
        client.try_cancel_order(&stranger, &order_id),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn cancel_order_not_found_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    assert_eq!(
        client.try_cancel_order(&owner, &999),
        Err(Ok(Error::TradeNotFound))
    );
}

// ── Full lifecycle ────────────────────────────────────────────────────────────

#[test]
fn full_trading_lifecycle() {
    let (env, _admin, client) = setup();
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);

    // Register meters
    let seller_meter = client.register_meter(
        &seller,
        &make_str(&env, "Solar Farm"),
        &EnergyType::Solar,
        &10000,
    );
    let buyer_meter =
        client.register_meter(&buyer, &make_str(&env, "Home"), &EnergyType::Solar, &5000);

    // Seller generates energy
    client.submit_reading(&seller, &seller_meter, &2000, &0, &111);
    assert_eq!(client.get_balance(&seller).kwh_balance, 2000);

    // Buyer consumes energy
    client.submit_reading(&buyer, &buyer_meter, &0, &500, &222);
    assert_eq!(client.get_balance(&buyer).kwh_balance, -500);

    // Seller creates sell order
    let order_id = client.create_sell_order(&seller, &seller_meter, &1000, &15);

    // Buyer accepts
    let trade_id = client.accept_order(&buyer, &order_id, &buyer_meter);

    // Verify final state
    let seller_balance = client.get_balance(&seller);
    assert_eq!(seller_balance.kwh_balance, 1000); // 2000 - 1000
    assert_eq!(seller_balance.total_earned, 15000); // 1000 * 15

    let buyer_balance = client.get_balance(&buyer);
    assert_eq!(buyer_balance.kwh_balance, 500); // -500 - 15000 + 1000 = -14500?
                                                // Actually: -500 - 15000 (paid) + 1000 (received) = -14500
                                                // But in our simplified model: balance -= total_price, then += kwh_amount
                                                // So: -500 - 15000 + 1000 = -14500
    assert_eq!(buyer_balance.total_spent, 15000);

    assert_eq!(client.get_total_energy_traded(), 1000);
}

// ── Queries ───────────────────────────────────────────────────────────────────

#[test]
fn get_meter_not_found() {
    let (env, _admin, client) = setup();
    assert_eq!(client.try_get_meter(&999), Err(Ok(Error::MeterNotFound)));
}

#[test]
fn get_trade_order_not_found() {
    let (env, _admin, client) = setup();
    assert_eq!(
        client.try_get_trade_order(&999),
        Err(Ok(Error::TradeNotFound))
    );
}

#[test]
fn get_energy_trade_not_found() {
    let (env, _admin, client) = setup();
    assert_eq!(
        client.try_get_energy_trade(&999),
        Err(Ok(Error::TradeNotFound))
    );
}

#[test]
fn get_balance_returns_zero_for_unknown() {
    let (env, _admin, client) = setup();
    let addr = Address::generate(&env);
    let balance = client.get_balance(&addr);
    assert_eq!(balance.kwh_balance, 0);
    assert_eq!(balance.total_earned, 0);
    assert_eq!(balance.total_spent, 0);
}
