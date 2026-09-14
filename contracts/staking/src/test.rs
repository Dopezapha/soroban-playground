#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env,
};

fn setup_test() -> (
    Env,
    StakingClient<'static>,
    Address,
    Address,
    Address,
    TokenClient<'static>,
    StellarAssetClient<'static>,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_addr = token.address();
    let sac = StellarAssetClient::new(&env, &token_addr);
    let token_client = TokenClient::new(&env, &token_addr);

    sac.mint(&user, &1000i128);

    let contract_id = env.register_contract(None, Staking);
    let client = StakingClient::new(&env, &contract_id);

    (env, client, admin, user, token_addr, token_client, sac)
}

#[test]
fn test_stake_and_unstake() {
    let (env, client, admin, user, token_addr, token_client, _sac) = setup_test();

    let unstake_period = 604800; // 7 days
    client.initialize(&admin, &token_addr, &unstake_period);

    let shares = client.stake(&user, &500i128);
    assert_eq!(shares, 500);

    let request_idx = client.request_unstake(&user, &500i128);

    // Advance time to unlock period
    env.ledger().with_mut(|l| l.timestamp += unstake_period + 1);

    let amount = client.claim_unstake(&user, &request_idx);
    assert_eq!(amount, 500);
    assert_eq!(token_client.balance(&user), 1000);
}

#[test]
fn test_initialize_zero_period_fails() {
    let (_env, client, admin, _user, token_addr, _token_client, _sac) = setup_test();
    let res = client.try_initialize(&admin, &token_addr, &0);
    assert_eq!(res, Err(Ok(Error::InvalidWithdrawalPeriod)));
}

#[test]
fn test_already_initialized_fails() {
    let (_env, client, admin, _user, token_addr, _token_client, _sac) = setup_test();
    client.initialize(&admin, &token_addr, &604800);
    let res = client.try_initialize(&admin, &token_addr, &604800);
    assert_eq!(res, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn test_uninitialized_calls_fail() {
    let (_env, client, _admin, user, _token_addr, _token_client, _sac) = setup_test();

    assert_eq!(
        client.try_stake(&user, &100),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        client.try_request_unstake(&user, &100),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        client.try_claim_unstake(&user, &0),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        client.try_get_stake_info(&user),
        Err(Ok(Error::NotInitialized))
    );
}

#[test]
fn test_stake_zero_or_negative_amount_fails() {
    let (_env, client, admin, user, token_addr, _token_client, _sac) = setup_test();
    client.initialize(&admin, &token_addr, &604800);

    assert_eq!(client.try_stake(&user, &0), Err(Ok(Error::ZeroAmount)));
    assert_eq!(client.try_stake(&user, &-50), Err(Ok(Error::ZeroAmount)));
}

#[test]
fn test_request_unstake_zero_or_insufficient_fails() {
    let (_env, client, admin, user, token_addr, _token_client, _sac) = setup_test();
    client.initialize(&admin, &token_addr, &604800);

    client.stake(&user, &500i128);

    assert_eq!(
        client.try_request_unstake(&user, &0),
        Err(Ok(Error::ZeroAmount))
    );
    assert_eq!(
        client.try_request_unstake(&user, &600),
        Err(Ok(Error::InsufficientBalance))
    );
}

#[test]
fn test_claim_unstake_premature_fails() {
    let (env, client, admin, user, token_addr, _token_client, _sac) = setup_test();
    let unstake_period = 604800;
    client.initialize(&admin, &token_addr, &unstake_period);

    client.stake(&user, &500i128);
    let request_idx = client.request_unstake(&user, &500i128);

    // Advance time partially, but not enough
    env.ledger().with_mut(|l| l.timestamp += 100);

    let res = client.try_claim_unstake(&user, &request_idx);
    assert_eq!(res, Err(Ok(Error::InvalidWithdrawalPeriod)));
}

#[test]
fn test_double_claim_unstake_fails() {
    let (env, client, admin, user, token_addr, _token_client, _sac) = setup_test();
    let unstake_period = 604800;
    client.initialize(&admin, &token_addr, &unstake_period);

    client.stake(&user, &500i128);
    let request_idx = client.request_unstake(&user, &500i128);

    env.ledger().with_mut(|l| l.timestamp += unstake_period + 1);

    // First claim succeeds
    let amount = client.claim_unstake(&user, &request_idx);
    assert_eq!(amount, 500);

    // Second claim fails with AlreadyClaimed
    let res = client.try_claim_unstake(&user, &request_idx);
    assert_eq!(res, Err(Ok(Error::AlreadyClaimed)));
}

#[test]
fn test_get_admin_and_queries() {
    let (_env, client, admin, user, token_addr, _token_client, _sac) = setup_test();
    let unstake_period = 604800;
    client.initialize(&admin, &token_addr, &unstake_period);

    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_unstake_period(), unstake_period);
    assert_eq!(client.get_total_staked(), 0);
    assert_eq!(client.get_total_shares(), 0);

    client.stake(&user, &300i128);
    assert_eq!(client.get_total_staked(), 300);
    assert_eq!(client.get_total_shares(), 300);

    let info = client.get_stake_info(&user);
    assert_eq!(info.amount, 300);
    assert_eq!(info.shares, 300);
}
