// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{Address, Env};

use crate::types::{
    DataKey, EnergyBalance, EnergyTrade, Error, InstanceKey, MeterReading, SmartMeter, TradeOrder,
};

// ── Admin / init ──────────────────────────────────────────────────────────────

pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&InstanceKey::Admin)
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&InstanceKey::Admin, admin);
}

pub fn get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&InstanceKey::Admin)
        .ok_or(Error::NotInitialized)
}

// ── Counters ──────────────────────────────────────────────────────────────────

pub fn next_meter_id(env: &Env) -> u32 {
    let id: u32 = env
        .storage()
        .instance()
        .get(&InstanceKey::MeterCount)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&InstanceKey::MeterCount, &id);
    id
}

pub fn get_meter_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::MeterCount)
        .unwrap_or(0)
}

pub fn next_trade_id(env: &Env) -> u32 {
    let id: u32 = env
        .storage()
        .instance()
        .get(&InstanceKey::TradeCount)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&InstanceKey::TradeCount, &id);
    id
}

pub fn get_trade_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::TradeCount)
        .unwrap_or(0)
}

pub fn set_total_energy_traded(env: &Env, amount: i128) {
    env.storage()
        .instance()
        .set(&InstanceKey::TotalEnergyTraded, &amount);
}

pub fn get_total_energy_traded(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&InstanceKey::TotalEnergyTraded)
        .unwrap_or(0)
}

// ── Smart Meters ──────────────────────────────────────────────────────────────

pub fn set_meter(env: &Env, id: u32, meter: &SmartMeter) {
    env.storage().persistent().set(&DataKey::Meter(id), meter);
}

pub fn get_meter(env: &Env, id: u32) -> Result<SmartMeter, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Meter(id))
        .ok_or(Error::MeterNotFound)
}

// ── Meter Readings ────────────────────────────────────────────────────────────

pub fn set_meter_reading(env: &Env, meter_id: u32, timestamp: u64, reading: &MeterReading) {
    env.storage()
        .persistent()
        .set(&DataKey::MeterReading(meter_id, timestamp), reading);
}

pub fn get_meter_reading(env: &Env, meter_id: u32, timestamp: u64) -> Option<MeterReading> {
    env.storage()
        .persistent()
        .get(&DataKey::MeterReading(meter_id, timestamp))
}

// ── Trade Orders ──────────────────────────────────────────────────────────────

pub fn set_trade_order(env: &Env, id: u32, order: &TradeOrder) {
    env.storage().persistent().set(&DataKey::Trade(id), order);
}

pub fn get_trade_order(env: &Env, id: u32) -> Result<TradeOrder, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Trade(id))
        .ok_or(Error::TradeNotFound)
}

// ── Energy Trades ─────────────────────────────────────────────────────────────

pub fn set_energy_trade(env: &Env, id: u32, trade: &EnergyTrade) {
    env.storage().persistent().set(&DataKey::Trade(id), trade);
}

pub fn get_energy_trade(env: &Env, id: u32) -> Result<EnergyTrade, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Trade(id))
        .ok_or(Error::TradeNotFound)
}

// ── Energy Balances ───────────────────────────────────────────────────────────

pub fn set_balance(env: &Env, addr: &Address, balance: &EnergyBalance) {
    env.storage()
        .persistent()
        .set(&DataKey::Balance(addr.clone()), balance);
}

pub fn get_balance(env: &Env, addr: &Address) -> EnergyBalance {
    env.storage()
        .persistent()
        .get(&DataKey::Balance(addr.clone()))
        .unwrap_or(EnergyBalance {
            address: addr.clone(),
            kwh_balance: 0,
            total_earned: 0,
            total_spent: 0,
        })
}
