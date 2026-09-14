#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, token, Address, Env, String};

#[test]
fn test_parametric_crop_insurance_flow() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let farmer = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_id = token_contract.address();
    let token_client = token::Client::new(&env, &token_id);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

    let insurance_id = env.register_contract(None, CropInsurance);
    token_admin_client.mint(&farmer, &1000);
    token_admin_client.mint(&insurance_id, &5000);

    let client = CropInsuranceClient::new(&env, &insurance_id);

    client.initialize(&admin, &oracle, &token_id);

    let region = String::from_str(&env, "REGION_AFRICA_01");

    // Create policy: drought trigger if rainfall < 50mm, premium 100, payout 500
    let policy_id = client.create_policy(
        &farmer, &region, &50, &true, // trigger on drought
        &100, &500, &86400,
    );

    let policy = client.get_policy(&policy_id);
    assert_eq!(policy.id, policy_id);
    assert_eq!(policy.status, PolicyStatus::Active);
    assert_eq!(token_client.balance(&farmer), 900);

    // Oracle submits 30mm rainfall (drought condition)
    client.submit_rainfall_data(&oracle, &region, &30);
    assert_eq!(client.get_rainfall(&region), Some(30));

    // Trigger payout
    let payout = client.trigger_payout(&policy_id);
    assert_eq!(payout, 500);

    let updated_policy = client.get_policy(&policy_id);
    assert_eq!(updated_policy.status, PolicyStatus::Claimed);
}
