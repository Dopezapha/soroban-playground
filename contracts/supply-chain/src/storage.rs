// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{Address, Env};

use crate::types::{
    Checkpoint, ColdChainSla, DataKey, Error, InstanceKey, PenaltyRecord, Product, QualityReport,
    TemperatureLog,
};

pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&InstanceKey::Admin)
}
pub fn set_admin(env: &Env, a: &Address) {
    env.storage().instance().set(&InstanceKey::Admin, a);
}
pub fn get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&InstanceKey::Admin)
        .ok_or(Error::NotInitialized)
}

pub fn get_product_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::ProductCount)
        .unwrap_or(0)
}
pub fn set_product_count(env: &Env, v: u32) {
    env.storage().instance().set(&InstanceKey::ProductCount, &v);
}

// ── Products ──────────────────────────────────────────────────────────────────

pub fn set_product(env: &Env, p: &Product) {
    env.storage().persistent().set(&DataKey::Product(p.id), p);
}
pub fn get_product(env: &Env, id: u32) -> Result<Product, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Product(id))
        .ok_or(Error::ProductNotFound)
}

// ── Checkpoints ───────────────────────────────────────────────────────────────

pub fn get_checkpoint_count(env: &Env, product_id: u32) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::CheckpointCount(product_id))
        .unwrap_or(0)
}
pub fn set_checkpoint_count(env: &Env, product_id: u32, v: u32) {
    env.storage()
        .persistent()
        .set(&DataKey::CheckpointCount(product_id), &v);
}
pub fn set_checkpoint(env: &Env, c: &Checkpoint) {
    env.storage()
        .persistent()
        .set(&DataKey::Checkpoint(c.product_id, c.index), c);
}
pub fn get_checkpoint(env: &Env, product_id: u32, index: u32) -> Option<Checkpoint> {
    env.storage()
        .persistent()
        .get(&DataKey::Checkpoint(product_id, index))
}

// ── Quality reports ───────────────────────────────────────────────────────────

pub fn set_quality_report(env: &Env, r: &QualityReport) {
    env.storage()
        .persistent()
        .set(&DataKey::QualityReport(r.product_id), r);
}
pub fn get_quality_report(env: &Env, product_id: u32) -> Result<QualityReport, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::QualityReport(product_id))
        .ok_or(Error::QualityReportNotFound)
}

// ── Role management ───────────────────────────────────────────────────────────

pub fn is_inspector(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Inspector(addr.clone()))
        .unwrap_or(false)
}
pub fn set_inspector(env: &Env, addr: &Address, v: bool) {
    env.storage()
        .persistent()
        .set(&DataKey::Inspector(addr.clone()), &v);
}
pub fn is_handler(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Handler(addr.clone()))
        .unwrap_or(false)
}
pub fn set_handler(env: &Env, addr: &Address, v: bool) {
    env.storage()
        .persistent()
        .set(&DataKey::Handler(addr.clone()), &v);
}

// ── Cold Chain SLA ────────────────────────────────────────────────────────────

pub fn get_sla_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::SlaCount)
        .unwrap_or(0)
}
pub fn set_sla_count(env: &Env, v: u32) {
    env.storage().instance().set(&InstanceKey::SlaCount, &v);
}
pub fn next_sla_id(env: &Env) -> u32 {
    let id = get_sla_count(env) + 1;
    set_sla_count(env, id);
    id
}

pub fn set_cold_chain_sla(env: &Env, id: u32, sla: &ColdChainSla) {
    env.storage()
        .persistent()
        .set(&DataKey::ColdChainSla(id), sla);
}
pub fn get_cold_chain_sla(env: &Env, id: u32) -> Result<ColdChainSla, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::ColdChainSla(id))
        .ok_or(Error::SlaNotFound)
}

// ── Temperature Logs ──────────────────────────────────────────────────────────

pub fn set_temperature_log(env: &Env, product_id: u32, timestamp: u64, log: &TemperatureLog) {
    env.storage()
        .persistent()
        .set(&DataKey::TemperatureLog(product_id, timestamp), log);
}
pub fn get_temperature_log(env: &Env, product_id: u32, timestamp: u64) -> Option<TemperatureLog> {
    env.storage()
        .persistent()
        .get(&DataKey::TemperatureLog(product_id, timestamp))
}

// ── Penalty Records ───────────────────────────────────────────────────────────

pub fn get_penalty_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::PenaltyCount)
        .unwrap_or(0)
}
pub fn set_penalty_count(env: &Env, v: u32) {
    env.storage().instance().set(&InstanceKey::PenaltyCount, &v);
}
pub fn next_penalty_id(env: &Env) -> u32 {
    let id = get_penalty_count(env) + 1;
    set_penalty_count(env, id);
    id
}

pub fn set_penalty_record(env: &Env, id: u32, penalty: &PenaltyRecord) {
    env.storage()
        .persistent()
        .set(&DataKey::Penalty(id), penalty);
}
pub fn get_penalty_record(env: &Env, id: u32) -> Result<PenaltyRecord, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Penalty(id))
        .ok_or(Error::PenaltyNotFound)
}
