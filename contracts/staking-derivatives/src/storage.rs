// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{Address, Env};

use crate::types::{DataKey, Error, InstanceKey, UnbondEntry};

macro_rules! instance_get {
    ($fn:ident, $key:ident, $t:ty, $default:expr) => {
        pub fn $fn(env: &Env) -> $t {
            env.storage()
                .instance()
                .get(&InstanceKey::$key)
                .unwrap_or($default)
        }
    };
}
macro_rules! instance_set {
    ($fn:ident, $key:ident, $t:ty) => {
        pub fn $fn(env: &Env, v: $t) {
            env.storage().instance().set(&InstanceKey::$key, &v);
        }
    };
}

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

instance_get!(get_total_staked, TotalStaked, i128, 0);
instance_set!(set_total_staked, TotalStaked, i128);
instance_get!(get_total_lst, TotalLst, i128, 0);
instance_set!(set_total_lst, TotalLst, i128);
instance_get!(get_exchange_rate, ExchangeRate, i128, 1_000_000);
instance_set!(set_exchange_rate, ExchangeRate, i128);
instance_get!(get_last_accrual_ts, LastAccrualTs, u64, 0);
instance_set!(set_last_accrual_ts, LastAccrualTs, u64);
instance_get!(get_reward_rate_bps, RewardRateBps, i128, 500);
instance_set!(set_reward_rate_bps, RewardRateBps, i128);
instance_get!(get_unbonding_period, UnbondingPeriod, u64, 604_800);
instance_set!(set_unbonding_period, UnbondingPeriod, u64);
instance_get!(is_paused, Paused, bool, false);
instance_set!(set_paused, Paused, bool);
instance_get!(get_total_rewards, TotalRewards, i128, 0);
instance_set!(set_total_rewards, TotalRewards, i128);
instance_get!(get_total_unbonding, TotalUnbonding, i128, 0);
instance_set!(set_total_unbonding, TotalUnbonding, i128);

// ── Per-user state ────────────────────────────────────────────────────────────

pub fn get_lst_balance(env: &Env, addr: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::LstBalance(addr.clone()))
        .unwrap_or(0)
}

pub fn set_lst_balance(env: &Env, addr: &Address, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::LstBalance(addr.clone()), &amount);
}

pub fn get_unbond_count(env: &Env, addr: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::UnbondCount(addr.clone()))
        .unwrap_or(0)
}

pub fn set_unbond_count(env: &Env, addr: &Address, count: u32) {
    env.storage()
        .persistent()
        .set(&DataKey::UnbondCount(addr.clone()), &count);
}

pub fn get_unbond_entry(env: &Env, addr: &Address, idx: u32) -> Option<UnbondEntry> {
    env.storage()
        .persistent()
        .get(&DataKey::UnbondEntry(addr.clone(), idx))
}

pub fn set_unbond_entry(env: &Env, addr: &Address, idx: u32, entry: &UnbondEntry) {
    env.storage()
        .persistent()
        .set(&DataKey::UnbondEntry(addr.clone(), idx), entry);
}

// ── Validator stakes ──────────────────────────────────────────────────────────

pub fn get_validator_stake(env: &Env, validator: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::ValidatorStake(validator.clone()))
        .unwrap_or(0)
}

pub fn set_validator_stake(env: &Env, validator: &Address, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::ValidatorStake(validator.clone()), &amount);
}
