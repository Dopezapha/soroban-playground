#![cfg(test)]

use super::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Bytes, BytesN};

fn setup_env<'a>() -> (Env, BountyDisputeContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, BountyDisputeContract);
    let client = BountyDisputeContractClient::new(&env, &contract_id);
    let whitehat = Address::generate(&env);
    (env, client, whitehat)
}

fn commit_hash(env: &Env) -> Bytes {
    Bytes::from_slice(env, &[7u8; 32])
}

#[test]
fn test_submit_and_reveal_flow() {
    let (env, client, whitehat) = setup_env();

    let id = client.submit_vulnerability(&1u64, &commit_hash(&env), &whitehat);
    assert_eq!(id, 1);

    // second submission gets a different id
    let id2 = client.submit_vulnerability(&1u64, &commit_hash(&env), &whitehat);
    assert_eq!(id2, 2);

    let ids = client.submissions_for_bounty(&1u64);
    assert_eq!(ids.len(), 2);
}

#[test]
fn test_bonded_arbitrator_can_rule() {
    let (env, client, _w) = setup_env();
    let arbitrator = Address::generate(&env);

    // Not bonded -> cannot rule
    let res = client.try_resolve_bounty_dispute(&1u64, &RulingVerdict::PayWhitehat, &arbitrator);
    assert!(res.is_err());

    // Bond and rule
    client.stake_arbitrator_bond(&arbitrator, &1_000_000i128);
    client.resolve_bounty_dispute(&1u64, &RulingVerdict::PayWhitehat, &arbitrator);

    let r = client.get_ruling(&1u64).unwrap();
    assert!(r.pay_whitehat);
    assert!(!client.is_final(&1u64)); // appeal window still active
}

#[test]
fn test_double_stake_fails() {
    let (_env, client, _w) = setup_env();
    let arbitrator = Address::generate(&env_placeholder());
    let _ = arbitrator;
    let _ = client;
}

// helper to keep compiler happy in placeholder test above
fn env_placeholder() -> soroban_sdk::Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

#[test]
fn test_real_double_stake_fails() {
    let (env, client, _w) = setup_env();
    let arbitrator = Address::generate(&env);
    client.stake_arbitrator_bond(&arbitrator, &1_000_000i128);
    let res = client.try_stake_arbitrator_bond(&arbitrator, &1_000_000i128);
    assert!(matches!(res, Err(Ok(DisputeError::AlreadyStaked))));
}

#[test]
fn test_unbonded_ruling_fails() {
    let (env, client, _w) = setup_env();
    let arbitrator = Address::generate(&env);
    let res =
        client.try_resolve_bounty_dispute(&9u64, &RulingVerdict::ReturnToSponsor, &arbitrator);
    assert!(matches!(res, Err(Ok(DisputeError::NotBonded))));
}
