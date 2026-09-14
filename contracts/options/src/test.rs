// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Env,
};

/// PRICE_PRECISION = 1_000_000, so spot = 100_000_000 means $100.00
const SPOT: i128 = 100_000_000; // $100
const STRIKE: i128 = 100_000_000; // ATM $100
const SIZE: i128 = 1_000_000; // 1 unit
const PREMIUM: i128 = 5_000_000; // $5 premium
const MARGIN: i128 = 25_000_000; // $25 margin (25% of notional = $100)

fn setup() -> (Env, Address, Address, OptionsContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, OptionsContract);
    let client = OptionsContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    client.initialize(&admin);
    // Set initial timestamp
    env.ledger().with_mut(|l| l.timestamp = 1_000_000);
    (env, admin, writer, client)
}

fn write_call_option(env: &Env, client: &OptionsContractClient, writer: &Address) -> u32 {
    let expiry = env.ledger().timestamp() + 86_400 * 30; // 30 days
    client.write_option(
        writer,
        &OptionType::Call,
        &STRIKE,
        &SPOT,
        &SIZE,
        &PREMIUM,
        &expiry,
        &MARGIN,
    )
}

#[test]
fn test_initialize() {
    let (_env, _admin, _writer, client) = setup();
    assert_eq!(client.get_option_count(), 0);
}

#[test]
fn test_double_init_fails() {
    let (_env, admin, _writer, client) = setup();
    let result = client.try_initialize(&admin);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn test_write_call_option() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    assert_eq!(id, 0);
    assert_eq!(client.get_option_count(), 1);

    let opt = client.get_option(&0);
    assert_eq!(opt.strike_price, STRIKE);
    assert_eq!(opt.size, SIZE);
    assert!(matches!(opt.option_type, OptionType::Call));
    assert!(matches!(opt.status, OptionStatus::Active));
}

#[test]
fn test_write_option_insufficient_margin_fails() {
    let (env, _admin, writer, client) = setup();
    let expiry = env.ledger().timestamp() + 86_400 * 30;
    // MIN_MARGIN = 20% of notional = 20% of $100 = $20_000_000
    let low_margin = 1_000_000; // Only $1 — too low
    let result = client.try_write_option(
        &writer,
        &OptionType::Call,
        &STRIKE,
        &SPOT,
        &SIZE,
        &PREMIUM,
        &expiry,
        &low_margin,
    );
    assert_eq!(result, Err(Ok(Error::InsufficientMargin)));
}

#[test]
fn test_write_expired_option_fails() {
    let (env, _admin, writer, client) = setup();
    let past_expiry = env.ledger().timestamp() - 1;
    let result = client.try_write_option(
        &writer,
        &OptionType::Call,
        &STRIKE,
        &SPOT,
        &SIZE,
        &PREMIUM,
        &past_expiry,
        &MARGIN,
    );
    assert_eq!(result, Err(Ok(Error::InvalidExpiry)));
}

#[test]
fn test_buy_option() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let buyer = Address::generate(&env);
    let premium_paid = client.buy_option(&buyer, &id);
    assert_eq!(premium_paid, PREMIUM);

    let opt = client.get_option(&id);
    assert!(opt.holder.is_some());
}

#[test]
fn test_exercise_itm_call() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let buyer = Address::generate(&env);
    client.buy_option(&buyer, &id);

    let opt = client.get_option(&id);
    // Advance to expiry
    env.ledger().with_mut(|l| l.timestamp = opt.expiry);

    // Settlement price = $110 (10% above strike) → payout = $10 * 1 unit
    let settlement = 110_000_000_i128; // $110
    let payout = client.exercise(&buyer, &id, &settlement);

    // payout = (110_000_000 - 100_000_000) * 1_000_000 / 1_000_000 = 10_000_000
    assert_eq!(payout, 10_000_000);

    let opt = client.get_option(&id);
    assert!(matches!(opt.status, OptionStatus::Exercised));
}

#[test]
fn test_exercise_otm_call() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let buyer = Address::generate(&env);
    client.buy_option(&buyer, &id);

    let opt = client.get_option(&id);
    env.ledger().with_mut(|l| l.timestamp = opt.expiry);

    // Settlement at $90 (below strike) → payout = 0
    let payout = client.exercise(&buyer, &id, &90_000_000);
    assert_eq!(payout, 0);
}

#[test]
fn test_exercise_before_expiry_fails() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let buyer = Address::generate(&env);
    client.buy_option(&buyer, &id);

    let result = client.try_exercise(&buyer, &id, &110_000_000);
    assert_eq!(result, Err(Ok(Error::OptionNotExpired)));
}

#[test]
fn test_exercise_wrong_holder_fails() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let buyer = Address::generate(&env);
    client.buy_option(&buyer, &id);

    let opt = client.get_option(&id);
    env.ledger().with_mut(|l| l.timestamp = opt.expiry);

    let impostor = Address::generate(&env);
    let result = client.try_exercise(&impostor, &id, &110_000_000);
    assert_eq!(result, Err(Ok(Error::NotOptionHolder)));
}

#[test]
fn test_expire_option() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let opt = client.get_option(&id);
    env.ledger().with_mut(|l| l.timestamp = opt.expiry);

    // Writer expires unexercised option
    client.expire_option(&writer, &id);
    let opt = client.get_option(&id);
    assert!(matches!(opt.status, OptionStatus::Expired));
}

#[test]
fn test_margin_deposit_and_withdraw() {
    let (env, _admin, writer, client) = setup();
    client.deposit_margin(&writer, &50_000_000);
    let bal = client.get_margin_balance(&writer);
    assert_eq!(bal, 50_000_000);

    client.withdraw_margin(&writer, &20_000_000);
    let bal = client.get_margin_balance(&writer);
    assert_eq!(bal, 30_000_000);
}

#[test]
fn test_withdraw_more_than_balance_fails() {
    let (env, _admin, writer, client) = setup();
    client.deposit_margin(&writer, &10_000_000);
    let result = client.try_withdraw_margin(&writer, &50_000_000);
    assert_eq!(result, Err(Ok(Error::InsufficientMargin)));
}

#[test]
fn test_check_margin_call() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    let req = client.check_margin(&id);
    // Just enough margin was deposited
    assert!(!req.margin_call);
    assert_eq!(req.deposited, MARGIN);
}

#[test]
fn test_compute_greeks_call() {
    let (env, _admin, writer, client) = setup();
    let id = write_call_option(&env, &client, &writer);
    // 20% implied volatility
    let greeks = client.compute_greeks(&id, &SPOT, &2000);
    // Delta for ATM call should be near 0.5 (500_000 in GREEK_PRECISION units)
    assert!(greeks.delta > 0);
    assert!(greeks.delta <= 1_000_000);
    // Gamma should be positive
    // Vega should be positive for long option
    // Time value should be positive (not yet expired)
}

#[test]
fn test_compute_greeks_put() {
    let (env, _admin, writer, client) = setup();
    let expiry = env.ledger().timestamp() + 86_400 * 30;
    let id = client.write_option(
        &writer,
        &OptionType::Put,
        &STRIKE,
        &SPOT,
        &SIZE,
        &PREMIUM,
        &expiry,
        &MARGIN,
    );
    let greeks = client.compute_greeks(&id, &SPOT, &2000);
    // Delta for ATM put should be near -0.5
    assert!(greeks.delta < 0 || greeks.delta == 0);
}

#[test]
fn test_itm_put_exercise() {
    let (env, _admin, writer, client) = setup();
    let expiry = env.ledger().timestamp() + 86_400 * 30;
    let id = client.write_option(
        &writer,
        &OptionType::Put,
        &STRIKE,
        &SPOT,
        &SIZE,
        &PREMIUM,
        &expiry,
        &MARGIN,
    );
    let buyer = Address::generate(&env);
    client.buy_option(&buyer, &id);
    env.ledger().with_mut(|l| l.timestamp = expiry);

    // Settlement at $85 → put ITM, payout = (100 - 85) * 1 = $15
    let payout = client.exercise(&buyer, &id, &85_000_000);
    assert_eq!(payout, 15_000_000);
}
