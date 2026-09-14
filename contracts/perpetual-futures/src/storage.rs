// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use crate::types::{Error, FundingRate, Position};
use soroban_sdk::{contracttype, Address, Env};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Initialized,
    Paused,
    PositionCount,
    Position(u64),
    FundingRate,
}

pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Initialized)
}

pub fn set_initialized(env: &Env) {
    env.storage().instance().set(&DataKey::Initialized, &true);
}

pub fn get_admin(env: &Env) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&DataKey::Paused, &paused);
}

pub fn get_position_count(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::PositionCount)
        .unwrap_or(0)
}

pub fn set_position_count(env: &Env, count: u64) {
    env.storage()
        .instance()
        .set(&DataKey::PositionCount, &count);
}

pub fn get_position(env: &Env, id: u64) -> Result<Position, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Position(id))
        .ok_or(Error::PositionNotFound)
}

pub fn set_position(env: &Env, position: &Position) {
    env.storage()
        .persistent()
        .set(&DataKey::Position(position.id), position);
}

pub fn get_funding_rate(env: &Env) -> Result<FundingRate, Error> {
    env.storage()
        .instance()
        .get(&DataKey::FundingRate)
        .ok_or(Error::NotInitialized)
}

pub fn set_funding_rate(env: &Env, rate: &FundingRate) {
    env.storage().instance().set(&DataKey::FundingRate, rate);
}
