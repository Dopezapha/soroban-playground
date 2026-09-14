// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, String};

use crate::types::{Error, ProductStatus, QualityResult};
use crate::{SupplyChain, SupplyChainClient};

fn setup() -> (Env, Address, SupplyChainClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, SupplyChain);
    let client = SupplyChainClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, admin, client)
}

#[test]
fn test_initialize_sets_admin() {
    let (_env, _admin, client) = setup();
    assert_eq!(client.product_count(), 0);
}

#[test]
fn test_initialize_twice_fails() {
    let (env, admin, client) = setup();
    let _ = env;
    let result = client.try_initialize(&admin);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn test_register_product_stores_data() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget A"), &12345u64);
    assert_eq!(id, 1);
    let product = client.get_product(&id);
    assert_eq!(product.id, 1);
    assert_eq!(product.metadata_hash, 12345u64);
    assert_eq!(product.status, ProductStatus::Registered);
}

#[test]
fn test_register_product_sequential_ids() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id1 = client.register_product(&owner, &String::from_str(&env, "A"), &1u64);
    let id2 = client.register_product(&owner, &String::from_str(&env, "B"), &2u64);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(client.product_count(), 2);
}

#[test]
fn test_register_product_empty_name_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let result = client.try_register_product(&owner, &String::from_str(&env, ""), &1u64);
    assert_eq!(result, Err(Ok(Error::EmptyName)));
}

#[test]
fn test_add_checkpoint_requires_handler() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    let non_handler = Address::generate(&env);
    let result = client.try_add_checkpoint(&non_handler, &id, &111u64, &222u64);
    assert_eq!(result, Err(Ok(Error::NotHandler)));
}

#[test]
fn test_add_checkpoint_updates_status() {
    let (env, admin, client) = setup();
    let handler = Address::generate(&env);
    client.add_handler(&admin, &handler);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    let cp_index = client.add_checkpoint(&handler, &id, &111u64, &222u64);
    assert_eq!(cp_index, 1);
    let product = client.get_product(&id);
    assert_eq!(product.status, ProductStatus::InTransit);
    assert_eq!(client.get_checkpoint_count(&id), 1);
}

#[test]
fn test_multiple_checkpoints_accumulate() {
    let (env, admin, client) = setup();
    let handler = Address::generate(&env);
    client.add_handler(&admin, &handler);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.add_checkpoint(&handler, &id, &1u64, &0u64);
    client.add_checkpoint(&handler, &id, &2u64, &0u64);
    client.add_checkpoint(&handler, &id, &3u64, &0u64);
    assert_eq!(client.get_checkpoint_count(&id), 3);
}

#[test]
fn test_quality_report_pass_sets_approved() {
    let (env, admin, client) = setup();
    let inspector = Address::generate(&env);
    client.add_inspector(&admin, &inspector);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.submit_quality_report(&inspector, &id, &QualityResult::Pass, &999u64);
    let product = client.get_product(&id);
    assert_eq!(product.status, ProductStatus::Approved);
    let report = client.get_quality_report(&id);
    assert_eq!(report.result, QualityResult::Pass);
    assert_eq!(report.report_hash, 999u64);
}

#[test]
fn test_quality_report_fail_sets_rejected() {
    let (env, admin, client) = setup();
    let inspector = Address::generate(&env);
    client.add_inspector(&admin, &inspector);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.submit_quality_report(&inspector, &id, &QualityResult::Fail, &0u64);
    assert_eq!(client.get_product(&id).status, ProductStatus::Rejected);
}

#[test]
fn test_quality_report_requires_inspector() {
    let (env, _admin, client) = setup();
    let non_inspector = Address::generate(&env);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    let result = client.try_submit_quality_report(&non_inspector, &id, &QualityResult::Pass, &0u64);
    assert_eq!(result, Err(Ok(Error::NotInspector)));
}

#[test]
fn test_recall_product() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.recall_product(&admin, &id);
    assert_eq!(client.get_product(&id).status, ProductStatus::Recalled);
}

#[test]
fn test_recall_twice_fails() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.recall_product(&admin, &id);
    let result = client.try_recall_product(&admin, &id);
    assert_eq!(result, Err(Ok(Error::AlreadyRecalled)));
}

#[test]
fn test_checkpoint_on_recalled_product_fails() {
    let (env, admin, client) = setup();
    let handler = Address::generate(&env);
    client.add_handler(&admin, &handler);
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.recall_product(&admin, &id);
    let result = client.try_add_checkpoint(&handler, &id, &1u64, &0u64);
    assert_eq!(result, Err(Ok(Error::AlreadyRecalled)));
}

#[test]
fn test_update_status_by_admin() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    client.update_status(&admin, &id, &ProductStatus::Delivered);
    assert_eq!(client.get_product(&id).status, ProductStatus::Delivered);
}

#[test]
fn test_update_status_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    let rando = Address::generate(&env);
    let result = client.try_update_status(&rando, &id, &ProductStatus::Delivered);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

#[test]
fn test_recall_unauthorized_fails() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Widget"), &1u64);
    let rando = Address::generate(&env);
    let result = client.try_recall_product(&rando, &id);
    assert_eq!(result, Err(Ok(Error::Unauthorized)));
}

// ── Cold Chain SLA ────────────────────────────────────────────────────────────

use crate::types::{ColdChainSla, SlaStatus, TemperatureLogStatus};

#[test]
fn test_create_cold_chain_sla() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    let sla_id = client.create_cold_chain_sla(
        &admin, &id, &2,     // min temp
        &8,     // max temp
        &30,    // max violation minutes
        &1000,  // penalty per violation
        &50000, // deposit
        &86400, // duration (1 day)
    );

    assert_eq!(sla_id, 1);
    assert_eq!(client.get_sla_count(), 1);

    let sla = client.get_cold_chain_sla(&sla_id);
    assert_eq!(sla.product_id, id);
    assert_eq!(sla.min_temp_celsius, 2);
    assert_eq!(sla.max_temp_celsius, 8);
    assert_eq!(sla.status, SlaStatus::Active);
}

#[test]
fn test_create_cold_chain_sla_invalid_range() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    assert_eq!(
        client.try_create_cold_chain_sla(
            &admin, &id, &10, // min > max
            &5,  // max
            &30, &1000, &50000, &86400,
        ),
        Err(Ok(Error::InvalidTemperatureRange))
    );
}

#[test]
fn test_log_temperature_normal() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    client.log_temperature(&recorder, &id, &5, &60, &12345);
    let log = client.get_temperature_log(&id, &env.ledger().timestamp());
    assert!(log.is_some());
    assert_eq!(log.unwrap().status, TemperatureLogStatus::Normal);
}

#[test]
fn test_log_temperature_violation() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    // Temperature below minimum
    client.log_temperature(&recorder, &id, &0, &60, &12345);

    let sla = client.get_cold_chain_sla(&1);
    assert_eq!(sla.violation_count, 1);
    assert_eq!(sla.total_penalties, 1000);

    let product = client.get_product(&id);
    assert_eq!(product.status, ProductStatus::TemperatureViolation);
}

#[test]
fn test_log_temperature_violation_above_max() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    // Temperature above maximum
    client.log_temperature(&recorder, &id, &15, &60, &12345);

    let sla = client.get_cold_chain_sla(&1);
    assert_eq!(sla.violation_count, 1);
}

#[test]
fn test_multiple_violations_accumulate() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    // Multiple violations
    client.log_temperature(&recorder, &id, &0, &60, &111);
    client.log_temperature(&recorder, &id, &15, &60, &222);

    let sla = client.get_cold_chain_sla(&1);
    assert_eq!(sla.violation_count, 2);
    assert_eq!(sla.total_penalties, 2000);
}

#[test]
fn test_penalty_count() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    client.log_temperature(&recorder, &id, &0, &60, &111);
    client.log_temperature(&recorder, &id, &15, &60, &222);

    assert_eq!(client.get_penalty_count(), 2);
}

#[test]
fn test_get_penalty_record() {
    let (env, admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    client.create_cold_chain_sla(&admin, &id, &2, &8, &30, &1000, &50000, &86400);

    client.log_temperature(&recorder, &id, &0, &60, &111);
    let penalty = client.get_penalty_record(&1);
    assert_eq!(penalty.product_id, id);
    assert_eq!(penalty.penalty_amount, 1000);
}

#[test]
fn test_no_sla_means_normal_temperature() {
    let (env, _admin, client) = setup();
    let owner = Address::generate(&env);
    let recorder = Address::generate(&env);
    let id = client.register_product(&owner, &String::from_str(&env, "Vaccine"), &1u64);

    // No SLA created - should be normal
    client.log_temperature(&recorder, &id, &50, &60, &12345);
    let log = client.get_temperature_log(&id, &env.ledger().timestamp());
    assert!(log.is_some());
    assert_eq!(log.unwrap().status, TemperatureLogStatus::Normal);
}
