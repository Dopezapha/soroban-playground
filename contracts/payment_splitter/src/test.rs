#![cfg(test)]

//! Tests for the Payment Splitter error handling (issue #999).
//!
//! Split arithmetic is exercised through `preview_split`, which needs no token
//! contract — every case below is about *which* error comes back and whether
//! the distribution conserves value.

use super::*;
use soroban_sdk::{testutils::Address as _, vec, Env};

fn setup() -> (Env, PaymentSplitterClient<'static>) {
    let env = Env::default();
    let contract_id = env.register_contract(None, PaymentSplitter);
    let client = PaymentSplitterClient::new(&env, &contract_id);
    (env, client)
}

fn addresses(env: &Env, n: u32) -> Vec<Address> {
    let mut v = Vec::new(env);
    for _ in 0..n {
        v.push_back(Address::generate(env));
    }
    v
}

// ─── Even and uneven distribution ────────────────────────────────────────────

#[test]
fn splits_evenly_when_divisible() {
    let (env, client) = setup();
    let recipients = addresses(&env, 4);

    let shares = client.preview_split(&100, &recipients);

    assert_eq!(shares, vec![&env, 25, 25, 25, 25]);
}

#[test]
fn distributes_remainder_instead_of_dropping_it() {
    let (env, client) = setup();
    let recipients = addresses(&env, 3);

    // 100 / 3 = 33 remainder 1. The old implementation dropped that unit.
    let shares = client.preview_split(&100, &recipients);

    assert_eq!(shares, vec![&env, 34, 33, 33]);
    assert_eq!(shares.iter().sum::<i128>(), 100, "no value may be lost");
}

#[test]
fn conserves_value_for_every_remainder() {
    let (env, client) = setup();
    let recipients = addresses(&env, 7);

    // Sweep every possible remainder for a 7-way split.
    for amount in 7..=64i128 {
        let shares = client.preview_split(&amount, &recipients);
        assert_eq!(
            shares.iter().sum::<i128>(),
            amount,
            "split of {amount} did not conserve value"
        );
        // No recipient may be starved while another is over-paid by more than
        // the single remainder unit.
        let min = shares.iter().min().unwrap();
        let max = shares.iter().max().unwrap();
        assert!(
            max - min <= 1,
            "shares for {amount} differ by more than one unit"
        );
    }
}

#[test]
fn single_recipient_receives_everything() {
    let (env, client) = setup();
    let recipients = addresses(&env, 1);

    assert_eq!(client.preview_split(&999, &recipients), vec![&env, 999]);
}

#[test]
fn smallest_valid_amount_gives_each_recipient_one_unit() {
    let (env, client) = setup();
    let recipients = addresses(&env, 5);

    assert_eq!(
        client.preview_split(&5, &recipients),
        vec![&env, 1, 1, 1, 1, 1]
    );
}

// ─── Error cases ─────────────────────────────────────────────────────────────

#[test]
fn rejects_zero_amount() {
    let (env, client) = setup();
    let recipients = addresses(&env, 2);

    assert_eq!(
        client.try_preview_split(&0, &recipients),
        Err(Ok(Error::ZeroAmount))
    );
}

#[test]
fn rejects_negative_amount() {
    let (env, client) = setup();
    let recipients = addresses(&env, 2);

    assert_eq!(
        client.try_preview_split(&-1, &recipients),
        Err(Ok(Error::ZeroAmount))
    );
}

#[test]
fn rejects_empty_recipient_list() {
    let (env, client) = setup();

    assert_eq!(
        client.try_preview_split(&100, &Vec::new(&env)),
        Err(Ok(Error::NoRecipients))
    );
}

#[test]
fn rejects_more_than_max_recipients() {
    let (env, client) = setup();
    let recipients = addresses(&env, MAX_RECIPIENTS + 1);

    assert_eq!(
        client.try_preview_split(&1_000_000, &recipients),
        Err(Ok(Error::TooManyRecipients))
    );
}

#[test]
fn accepts_exactly_max_recipients() {
    let (env, client) = setup();
    let recipients = addresses(&env, MAX_RECIPIENTS);

    // The boundary itself must be allowed — off-by-one here would silently cap
    // splits one recipient short.
    let shares = client.preview_split(&1_000_000, &recipients);
    assert_eq!(shares.len(), MAX_RECIPIENTS);
    assert_eq!(shares.iter().sum::<i128>(), 1_000_000);
}

#[test]
fn rejects_duplicate_recipients() {
    let (env, client) = setup();
    let duplicate = Address::generate(&env);
    let recipients = vec![&env, duplicate.clone(), Address::generate(&env), duplicate];

    // A duplicate would quietly receive two shares while the caller believes
    // it received one.
    assert_eq!(
        client.try_preview_split(&300, &recipients),
        Err(Ok(Error::DuplicateRecipient))
    );
}

#[test]
fn rejects_amount_smaller_than_recipient_count() {
    let (env, client) = setup();
    let recipients = addresses(&env, 10);

    // 9 / 10 == 0, so every recipient would get nothing and the call would
    // still report success.
    assert_eq!(
        client.try_preview_split(&9, &recipients),
        Err(Ok(Error::AmountTooSmall))
    );
}

#[test]
fn validation_order_reports_amount_before_recipients() {
    let (env, client) = setup();

    // Both inputs are invalid; the amount error is the more fundamental one and
    // must be the one surfaced, so the message stays stable as callers fix
    // their input.
    assert_eq!(
        client.try_preview_split(&0, &Vec::new(&env)),
        Err(Ok(Error::ZeroAmount))
    );
}

// ─── Transfer path ───────────────────────────────────────────────────────────

#[test]
fn rejects_payer_listed_as_recipient() {
    let env = Env::default();
    let contract_id = env.register_contract(None, PaymentSplitter);
    let client = PaymentSplitterClient::new(&env, &contract_id);

    let payer = Address::generate(&env);
    let token_id = Address::generate(&env);
    let recipients = vec![&env, Address::generate(&env), payer.clone()];

    env.mock_all_auths();

    assert_eq!(
        client.try_split(&payer, &token_id, &100, &recipients),
        Err(Ok(Error::PayerIsRecipient))
    );
}

#[test]
fn split_validates_before_requiring_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, PaymentSplitter);
    let client = PaymentSplitterClient::new(&env, &contract_id);

    let payer = Address::generate(&env);
    let token_id = Address::generate(&env);

    // No auth is mocked. A well-formed rejection proves validation ran first —
    // if require_auth came earlier this would panic instead of returning.
    assert_eq!(
        client.try_split(&payer, &token_id, &0, &addresses(&env, 2)),
        Err(Ok(Error::ZeroAmount))
    );
}
