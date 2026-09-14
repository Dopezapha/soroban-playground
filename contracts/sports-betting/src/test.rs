use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, String,
};

struct Fixture {
    env: Env,
    client: SportsBettingClient<'static>,
    token: Address,
    token_admin: StellarAssetClient<'static>,
    token_client: TokenClient<'static>,
    admin: Address,
    fee_recipient: Address,
    oracles: [Address; 3],
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(100);
    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_admin_address = Address::generate(&env);
    let asset = env.register_stellar_asset_contract_v2(token_admin_address);
    let token = asset.address();
    let token_admin = StellarAssetClient::new(&env, &token);
    let token_client = TokenClient::new(&env, &token);
    let contract = env.register_contract(None, SportsBetting);
    let client = SportsBettingClient::new(&env, &contract);
    client.initialize(&admin, &fee_recipient, &500);
    let oracles = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    for oracle in &oracles {
        client.add_oracle(oracle);
    }
    Fixture {
        env,
        client,
        token,
        token_admin,
        token_client,
        admin,
        fee_recipient,
        oracles,
    }
}

fn create_market(f: &Fixture, threshold: u32) -> u64 {
    f.client.create_market(
        &String::from_str(&f.env, "match-2026-001"),
        &f.token,
        &3,
        &200,
        &300,
        &threshold,
    )
}

fn fund(f: &Fixture, bettor: &Address, amount: i128) {
    f.token_admin.mint(bettor, &amount);
}

#[test]
fn initialization_and_market_validation() {
    let f = setup();
    assert_eq!(
        f.client.try_initialize(&f.admin, &f.fee_recipient, &500),
        Err(Ok(Error::AlreadyInitialized))
    );
    assert_eq!(
        f.client.try_create_market(
            &String::from_str(&f.env, "bad"),
            &f.token,
            &3,
            &200,
            &300,
            &4,
        ),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn escrow_and_dynamic_odds_are_correct() {
    let f = setup();
    let id = create_market(&f, 2);
    let home = Address::generate(&f.env);
    let away = Address::generate(&f.env);
    fund(&f, &home, 1_000);
    fund(&f, &away, 1_000);

    f.client.place_bet(&home, &id, &0, &100);
    f.client.place_bet(&away, &id, &2, &300);

    assert_eq!(f.token_client.balance(&home), 900);
    assert_eq!(f.client.get_bet(&id, &home, &0), 100);
    assert_eq!(f.client.get_market(&id).total_pool, 400);
    assert_eq!(f.client.odds(&id, &0), 38_000);
    assert_eq!(f.client.odds(&id, &1), 0);
}

#[test]
fn requires_multi_oracle_consensus_and_rejects_duplicate_vote() {
    let f = setup();
    let id = create_market(&f, 2);
    let bettor = Address::generate(&f.env);
    fund(&f, &bettor, 100);
    f.client.place_bet(&bettor, &id, &0, &100);
    f.env.ledger().set_timestamp(200);

    assert!(!f.client.submit_result(&f.oracles[0], &id, &0));
    assert_eq!(
        f.client.try_submit_result(&f.oracles[0], &id, &0),
        Err(Ok(Error::AlreadyVoted))
    );
    assert!(f.client.submit_result(&f.oracles[1], &id, &0));
    assert_eq!(f.client.get_market(&id).status, MarketStatus::Resolved);
}

#[test]
fn split_oracle_votes_do_not_settle() {
    let f = setup();
    let id = create_market(&f, 2);
    let bettor = Address::generate(&f.env);
    fund(&f, &bettor, 100);
    f.client.place_bet(&bettor, &id, &0, &100);
    f.env.ledger().set_timestamp(200);

    assert!(!f.client.submit_result(&f.oracles[0], &id, &0));
    assert!(!f.client.submit_result(&f.oracles[1], &id, &1));
    assert_eq!(f.client.get_market(&id).status, MarketStatus::Open);
}

#[test]
fn winners_receive_parimutuel_payout_and_fee_is_separate() {
    let f = setup();
    let id = create_market(&f, 2);
    let winner = Address::generate(&f.env);
    let loser = Address::generate(&f.env);
    fund(&f, &winner, 1_000);
    fund(&f, &loser, 1_000);
    f.client.place_bet(&winner, &id, &0, &100);
    f.client.place_bet(&loser, &id, &2, &300);
    f.env.ledger().set_timestamp(200);
    f.client.submit_result(&f.oracles[0], &id, &0);
    f.client.submit_result(&f.oracles[1], &id, &0);

    assert_eq!(f.client.claim(&winner, &id, &0), 380);
    assert_eq!(f.client.claim(&loser, &id, &2), 0);
    assert_eq!(f.token_client.balance(&winner), 1_280);
    assert_eq!(f.client.claim_fee(&id), 20);
    assert_eq!(f.token_client.balance(&f.fee_recipient), 20);
    assert_eq!(
        f.client.try_claim(&winner, &id, &0),
        Err(Ok(Error::NothingToClaim))
    );
    assert_eq!(f.client.try_claim_fee(&id), Err(Ok(Error::AlreadyClaimed)));
}

#[test]
fn expired_market_can_be_cancelled_and_refunded() {
    let f = setup();
    let id = create_market(&f, 3);
    let bettor = Address::generate(&f.env);
    fund(&f, &bettor, 500);
    f.client.place_bet(&bettor, &id, &1, &200);
    f.env.ledger().set_timestamp(301);

    f.client.cancel_expired(&id);
    assert_eq!(f.client.claim(&bettor, &id, &1), 200);
    assert_eq!(f.token_client.balance(&bettor), 500);
}

#[test]
fn outcome_without_stake_cancels_instead_of_locking_funds() {
    let f = setup();
    let id = create_market(&f, 2);
    let bettor = Address::generate(&f.env);
    fund(&f, &bettor, 100);
    f.client.place_bet(&bettor, &id, &0, &100);
    f.env.ledger().set_timestamp(200);
    f.client.submit_result(&f.oracles[0], &id, &2);
    f.client.submit_result(&f.oracles[1], &id, &2);

    assert_eq!(f.client.get_market(&id).status, MarketStatus::Cancelled);
    assert_eq!(f.client.claim(&bettor, &id, &0), 100);
}

#[test]
fn pause_blocks_new_risk_but_not_refunds() {
    let f = setup();
    let id = create_market(&f, 2);
    let bettor = Address::generate(&f.env);
    fund(&f, &bettor, 100);
    f.client.place_bet(&bettor, &id, &0, &100);
    f.client.set_paused(&true);
    assert_eq!(
        f.client.try_place_bet(&bettor, &id, &0, &1),
        Err(Ok(Error::Paused))
    );
    f.env.ledger().set_timestamp(301);
    f.client.cancel_expired(&id);
    assert_eq!(f.client.claim(&bettor, &id, &0), 100);
}
