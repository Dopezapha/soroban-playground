// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{Address, Env, String};

use crate::types::{DataKey, Error, InstanceKey, PauseAction, PauseProposal};

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

// ── Pause state ───────────────────────────────────────────────────────────────

pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&InstanceKey::Paused, &paused);
}

pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&InstanceKey::Paused)
        .unwrap_or(false)
}

pub fn set_pause_reason(env: &Env, reason: &String) {
    env.storage()
        .instance()
        .set(&InstanceKey::PauseReason, reason);
}

pub fn get_pause_reason(env: &Env) -> Option<String> {
    env.storage().instance().get(&InstanceKey::PauseReason)
}

pub fn set_pause_timestamp(env: &Env, timestamp: u64) {
    env.storage()
        .instance()
        .set(&InstanceKey::PauseTimestamp, &timestamp);
}

pub fn get_pause_timestamp(env: &Env) -> Option<u64> {
    env.storage().instance().get(&InstanceKey::PauseTimestamp)
}

// ── Threshold ─────────────────────────────────────────────────────────────────

pub fn set_threshold(env: &Env, threshold: u32) {
    env.storage()
        .instance()
        .set(&InstanceKey::Threshold, &threshold);
}

pub fn get_threshold(env: &Env) -> Result<u32, Error> {
    env.storage()
        .instance()
        .get(&InstanceKey::Threshold)
        .ok_or(Error::NotInitialized)
}

// ── Guardians ─────────────────────────────────────────────────────────────────

pub fn is_guardian(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Guardian(addr.clone()))
        .unwrap_or(false)
}

pub fn set_guardian(env: &Env, addr: &Address, is_guardian: bool) {
    env.storage()
        .persistent()
        .set(&DataKey::Guardian(addr.clone()), &is_guardian);
}

pub fn get_guardian_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::GuardianCount)
        .unwrap_or(0)
}

pub fn set_guardian_count(env: &Env, count: u32) {
    env.storage()
        .instance()
        .set(&InstanceKey::GuardianCount, &count);
}

// ── Proposals ─────────────────────────────────────────────────────────────────

pub fn next_proposal_id(env: &Env) -> u32 {
    let id: u32 = env
        .storage()
        .instance()
        .get(&InstanceKey::ProposalCount)
        .unwrap_or(0)
        + 1;
    env.storage()
        .instance()
        .set(&InstanceKey::ProposalCount, &id);
    id
}

pub fn get_proposal_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&InstanceKey::ProposalCount)
        .unwrap_or(0)
}

pub fn set_proposal(env: &Env, id: u32, proposal: &PauseProposal) {
    env.storage()
        .persistent()
        .set(&DataKey::Proposal(id), proposal);
}

pub fn get_proposal(env: &Env, id: u32) -> Result<PauseProposal, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id))
        .ok_or(Error::ProposalNotFound)
}
