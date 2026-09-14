#![no_std]

//! Parametric Crop Insurance with Satellite Rainfall Oracle Integration.
//!
//! Automated insurance policy creation, satellite rainfall oracle data updates,
//! and threshold-triggered parametric claim payouts on Soroban.

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, String,
};

const INSTANCE_TTL_THRESHOLD: u32 = 30 * 17_280;
const INSTANCE_TTL_BUMP: u32 = 120 * 17_280;
const DATA_TTL_THRESHOLD: u32 = 30 * 17_280;
const DATA_TTL_BUMP: u32 = 365 * 17_280;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Oracle,
    Token,
    Paused,
    PolicyCount,
    Policy(u64),
    Rainfall(String),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyStatus {
    Active,
    Claimed,
    Expired,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CropPolicy {
    pub id: u64,
    pub farmer: Address,
    pub region_id: String,
    pub rainfall_threshold_mm: u32,
    pub trigger_on_drought: bool,
    pub premium_amount: i128,
    pub payout_amount: i128,
    pub start_time: u64,
    pub end_time: u64,
    pub status: PolicyStatus,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    InvalidSchedule = 5,
    InvalidAmount = 6,
    PolicyNotFound = 7,
    PolicyNotActive = 8,
    TriggerConditionNotMet = 9,
    RainfallDataMissing = 10,
    AlreadyClaimed = 11,
    ArithmeticError = 12,
}

#[contract]
pub struct CropInsurance;

#[contractimpl]
impl CropInsurance {
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        token: Address,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::PolicyCount, &0u64);
        bump_instance(&env);
        env.events()
            .publish((symbol_short!("init"),), (admin, oracle, token));
        Ok(())
    }

    pub fn set_paused(env: Env, paused: bool) -> Result<(), Error> {
        admin(&env)?.require_auth();
        env.storage().instance().set(&DataKey::Paused, &paused);
        bump_instance(&env);
        env.events().publish((symbol_short!("paused"),), paused);
        Ok(())
    }

    pub fn set_oracle(env: Env, new_oracle: Address) -> Result<(), Error> {
        admin(&env)?.require_auth();
        env.storage().instance().set(&DataKey::Oracle, &new_oracle);
        bump_instance(&env);
        env.events().publish((symbol_short!("oracle"),), new_oracle);
        Ok(())
    }

    /// Create a parametric crop insurance policy funded by premium deposit.
    #[allow(clippy::too_many_arguments)]
    pub fn create_policy(
        env: Env,
        farmer: Address,
        region_id: String,
        rainfall_threshold_mm: u32,
        trigger_on_drought: bool,
        premium_amount: i128,
        payout_amount: i128,
        duration_seconds: u64,
    ) -> Result<u64, Error> {
        initialized(&env)?;
        not_paused(&env)?;
        farmer.require_auth();

        if premium_amount <= 0 || payout_amount <= premium_amount {
            return Err(Error::InvalidAmount);
        }
        if duration_seconds == 0 {
            return Err(Error::InvalidSchedule);
        }

        let now = env.ledger().timestamp();
        let end_time = now
            .checked_add(duration_seconds)
            .ok_or(Error::ArithmeticError)?;

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_addr).transfer(
            &farmer,
            &env.current_contract_address(),
            &premium_amount,
        );

        let id: u64 = env.storage().instance().get(&DataKey::PolicyCount).unwrap();
        let policy = CropPolicy {
            id,
            farmer,
            region_id,
            rainfall_threshold_mm,
            trigger_on_drought,
            premium_amount,
            payout_amount,
            start_time: now,
            end_time,
            status: PolicyStatus::Active,
        };

        put_policy(&env, &policy);
        env.storage()
            .instance()
            .set(&DataKey::PolicyCount, &(id + 1));
        bump_instance(&env);

        env.events().publish(
            (symbol_short!("create"), id),
            (policy.farmer, payout_amount),
        );
        Ok(id)
    }

    /// Update satellite rainfall oracle telemetry for a region.
    pub fn submit_rainfall_data(
        env: Env,
        oracle: Address,
        region_id: String,
        rainfall_mm: u32,
    ) -> Result<(), Error> {
        initialized(&env)?;
        not_paused(&env)?;
        oracle.require_auth();

        let expected_oracle: Address = env.storage().instance().get(&DataKey::Oracle).unwrap();
        if oracle != expected_oracle {
            return Err(Error::Unauthorized);
        }

        let key = DataKey::Rainfall(region_id.clone());
        env.storage().persistent().set(&key, &rainfall_mm);
        bump_key(&env, &key);

        env.events()
            .publish((symbol_short!("rain"), region_id), rainfall_mm);
        Ok(())
    }

    /// Trigger payout for a policy based on satellite oracle rainfall telemetry.
    pub fn trigger_payout(env: Env, policy_id: u64) -> Result<i128, Error> {
        initialized(&env)?;
        not_paused(&env)?;

        let mut policy = get_policy(&env, policy_id)?;
        if policy.status != PolicyStatus::Active {
            return Err(Error::PolicyNotActive);
        }

        let key = DataKey::Rainfall(policy.region_id.clone());
        let rainfall_mm: u32 = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::RainfallDataMissing)?;

        let condition_met = if policy.trigger_on_drought {
            rainfall_mm < policy.rainfall_threshold_mm
        } else {
            rainfall_mm > policy.rainfall_threshold_mm
        };

        if !condition_met {
            return Err(Error::TriggerConditionNotMet);
        }

        policy.status = PolicyStatus::Claimed;
        put_policy(&env, &policy);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &policy.farmer,
            &policy.payout_amount,
        );

        env.events().publish(
            (symbol_short!("claimed"), policy_id),
            (policy.farmer, policy.payout_amount),
        );
        Ok(policy.payout_amount)
    }

    /// Expire policy if duration ended without trigger condition being met.
    pub fn expire_policy(env: Env, policy_id: u64) -> Result<(), Error> {
        initialized(&env)?;
        let mut policy = get_policy(&env, policy_id)?;
        if policy.status != PolicyStatus::Active {
            return Err(Error::PolicyNotActive);
        }
        if env.ledger().timestamp() < policy.end_time {
            return Err(Error::InvalidSchedule);
        }

        policy.status = PolicyStatus::Expired;
        put_policy(&env, &policy);

        env.events()
            .publish((symbol_short!("expired"), policy_id), ());
        Ok(())
    }

    pub fn get_policy(env: Env, policy_id: u64) -> Result<CropPolicy, Error> {
        get_policy(&env, policy_id)
    }

    pub fn get_rainfall(env: Env, region_id: String) -> Option<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::Rainfall(region_id))
    }
}

fn initialized(env: &Env) -> Result<(), Error> {
    if !env.storage().instance().has(&DataKey::Admin) {
        return Err(Error::NotInitialized);
    }
    bump_instance(env);
    Ok(())
}

fn admin(env: &Env) -> Result<Address, Error> {
    initialized(env)?;
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(Error::NotInitialized)
}

fn not_paused(env: &Env) -> Result<(), Error> {
    if env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
    {
        return Err(Error::Paused);
    }
    Ok(())
}

fn get_policy(env: &Env, id: u64) -> Result<CropPolicy, Error> {
    initialized(env)?;
    let key = DataKey::Policy(id);
    let policy = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::PolicyNotFound)?;
    bump_key(env, &key);
    Ok(policy)
}

fn put_policy(env: &Env, policy: &CropPolicy) {
    let key = DataKey::Policy(policy.id);
    env.storage().persistent().set(&key, policy);
    bump_key(env, &key);
}

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_BUMP);
}

fn bump_key(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, DATA_TTL_THRESHOLD, DATA_TTL_BUMP);
}
