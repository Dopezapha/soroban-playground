// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, String};

use crate::types::{ClaimStatus, Error};
use crate::{WarrantyManagement, WarrantyManagementClient};

fn setup() -> (Env, Address, WarrantyManagementClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, WarrantyManagement);
    let client = WarrantyManagementClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, admin, client)
}

// ── Initialization ────────────────────────────────────────────────────────────

#[test]
fn test_initialize_ok() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, WarrantyManagement);
    let client = WarrantyManagementClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    assert_eq!(client.try_initialize(&admin), Ok(Ok(())));
}

#[test]
fn test_initialize_twice_fails() {
    let (_, admin, client) = setup();
    assert_eq!(
        client.try_initialize(&admin),
        Err(Ok(Error::AlreadyInitialized))
    );
}

// ── Pause / Unpause ───────────────────────────────────────────────────────────

#[test]
fn test_pause_unpause() {
    let (_, admin, client) = setup();
    assert!(!client.is_paused());
    client.pause(&admin);
    assert!(client.is_paused());
    client.unpause(&admin);
    assert!(!client.is_paused());
}

#[test]
fn test_pause_blocks_operations() {
    let (env, admin, client) = setup();
    client.pause(&admin);
    let manufacturer = Address::generate(&env);
    let result = client.try_register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    assert_eq!(result, Err(Ok(Error::ContractPaused)));
}

#[test]
fn test_unauthorized_pause() {
    let (env, _, client) = setup();
    let rando = Address::generate(&env);
    assert_eq!(client.try_pause(&rando), Err(Ok(Error::Unauthorized)));
}

// ── Product registration ──────────────────────────────────────────────────────

#[test]
fn test_register_product_ok() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget Pro"),
        &31_536_000u64,
    );
    assert_eq!(id, 1);
    let product = client.get_product(&id);
    assert_eq!(product.manufacturer, manufacturer);
    assert!(product.is_active);
}

#[test]
fn test_register_product_empty_name() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    assert_eq!(
        client.try_register_product(&manufacturer, &String::from_str(&env, ""), &31_536_000u64),
        Err(Ok(Error::EmptyName))
    );
}

#[test]
fn test_register_product_zero_duration() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    assert_eq!(
        client.try_register_product(&manufacturer, &String::from_str(&env, "Widget"), &0u64),
        Err(Ok(Error::InvalidDuration))
    );
}

#[test]
fn test_deactivate_product_by_manufacturer() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    client.deactivate_product(&manufacturer, &id);
    let product = client.get_product(&id);
    assert!(!product.is_active);
}

#[test]
fn test_deactivate_product_unauthorized() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let rando = Address::generate(&env);
    let id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    assert_eq!(
        client.try_deactivate_product(&rando, &id),
        Err(Ok(Error::Unauthorized))
    );
}

// ── Warranty issuance ─────────────────────────────────────────────────────────

#[test]
fn test_issue_warranty_ok() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let prod_id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    let war_id = client.issue_warranty(
        &manufacturer,
        &prod_id,
        &buyer,
        &String::from_str(&env, "SN-001"),
    );
    assert_eq!(war_id, 1);
    let warranty = client.get_warranty(&war_id);
    assert_eq!(warranty.owner, buyer);
    assert!(warranty.is_active);
}

#[test]
fn test_issue_warranty_inactive_product() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let prod_id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    client.deactivate_product(&manufacturer, &prod_id);
    assert_eq!(
        client.try_issue_warranty(
            &manufacturer,
            &prod_id,
            &buyer,
            &String::from_str(&env, "SN-001")
        ),
        Err(Ok(Error::ProductInactive))
    );
}

#[test]
fn test_issue_warranty_unauthorized() {
    let (env, _, client) = setup();
    let manufacturer = Address::generate(&env);
    let rando = Address::generate(&env);
    let buyer = Address::generate(&env);
    let prod_id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    assert_eq!(
        client.try_issue_warranty(&rando, &prod_id, &buyer, &String::from_str(&env, "SN-001")),
        Err(Ok(Error::Unauthorized))
    );
}

// ── Claim management ──────────────────────────────────────────────────────────

fn setup_with_warranty() -> (
    Env,
    Address,
    Address,
    u32,
    u32,
    WarrantyManagementClient<'static>,
) {
    let (env, _admin, client) = setup();
    let manufacturer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let prod_id = client.register_product(
        &manufacturer,
        &String::from_str(&env, "Widget"),
        &31_536_000u64,
    );
    let war_id = client.issue_warranty(
        &manufacturer,
        &prod_id,
        &buyer,
        &String::from_str(&env, "SN-001"),
    );
    (env, manufacturer, buyer, prod_id, war_id, client)
}

#[test]
fn test_file_claim_ok() {
    let (env, _, buyer, _, war_id, client) = setup_with_warranty();
    let claim_id = client.file_claim(&buyer, &war_id, &String::from_str(&env, "Screen cracked"));
    assert_eq!(claim_id, 1);
    let claim = client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Pending);
}

#[test]
fn test_file_claim_not_owner() {
    let (env, _, _, _, war_id, client) = setup_with_warranty();
    let rando = Address::generate(&env);
    assert_eq!(
        client.try_file_claim(&rando, &war_id, &String::from_str(&env, "Broken")),
        Err(Ok(Error::NotWarrantyOwner))
    );
}

#[test]
fn test_resolve_claim_approve() {
    let (env, manufacturer, buyer, _, war_id, client) = setup_with_warranty();
    let claim_id = client.file_claim(&buyer, &war_id, &String::from_str(&env, "Defective"));
    client.resolve_claim(&manufacturer, &claim_id, &true);
    let claim = client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Approved);
}

#[test]
fn test_resolve_claim_reject() {
    let (env, manufacturer, buyer, _, war_id, client) = setup_with_warranty();
    let claim_id = client.file_claim(&buyer, &war_id, &String::from_str(&env, "Defective"));
    client.resolve_claim(&manufacturer, &claim_id, &false);
    let claim = client.get_claim(&claim_id);
    assert_eq!(claim.status, ClaimStatus::Rejected);
}

#[test]
fn test_resolve_claim_already_processed() {
    let (env, manufacturer, buyer, _, war_id, client) = setup_with_warranty();
    let claim_id = client.file_claim(&buyer, &war_id, &String::from_str(&env, "Defective"));
    client.resolve_claim(&manufacturer, &claim_id, &true);
    assert_eq!(
        client.try_resolve_claim(&manufacturer, &claim_id, &false),
        Err(Ok(Error::ClaimAlreadyProcessed))
    );
}

#[test]
fn test_resolve_claim_unauthorized() {
    let (env, _, buyer, _, war_id, client) = setup_with_warranty();
    let rando = Address::generate(&env);
    let claim_id = client.file_claim(&buyer, &war_id, &String::from_str(&env, "Defective"));
    assert_eq!(
        client.try_resolve_claim(&rando, &claim_id, &true),
        Err(Ok(Error::Unauthorized))
    );
}

// ── Counters ──────────────────────────────────────────────────────────────────

#[test]
fn test_counters() {
    let (env, _, buyer, _, war_id, client) = setup_with_warranty();
    assert_eq!(client.product_count(), 1);
    assert_eq!(client.warranty_count(), 1);
    client.file_claim(&buyer, &war_id, &String::from_str(&env, "Broken"));
    assert_eq!(client.claim_count(), 1);
}
