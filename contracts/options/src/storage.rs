// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{Address, Env};

use crate::types::{DataKey, Error, InstanceKey, OptionContract};

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

instance_get!(get_option_count, OptionCount, u32, 0);
instance_set!(set_option_count, OptionCount, u32);
instance_get!(get_total_margin, TotalMargin, i128, 0);
instance_set!(set_total_margin, TotalMargin, i128);
instance_get!(is_paused, Paused, bool, false);
instance_set!(set_paused, Paused, bool);

// ── Option storage ────────────────────────────────────────────────────────────

pub fn get_option(env: &Env, id: u32) -> Option<OptionContract> {
    env.storage().persistent().get(&DataKey::Option(id))
}

pub fn set_option(env: &Env, opt: &OptionContract) {
    env.storage()
        .persistent()
        .set(&DataKey::Option(opt.id), opt);
}

// ── Margin storage ────────────────────────────────────────────────────────────

pub fn get_margin(env: &Env, writer: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Margin(writer.clone()))
        .unwrap_or(0)
}

pub fn set_margin(env: &Env, writer: &Address, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::Margin(writer.clone()), &amount);
}
