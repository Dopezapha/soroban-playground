// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! Recurring subscription billing backed by a subscriber's pre-approved
//! SEP-41 token allowance.
//!
//! Plans are immutable so a merchant cannot raise the price after consent.
//! A subscription additionally caps both its lifetime and number of charges.
//! Keepers can execute one due charge at a time, but only the subscriber or
//! merchant can cancel, and no operation can reactivate a cancelled record.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, String,
};

const INSTANCE_TTL_THRESHOLD: u32 = 120_960;
const INSTANCE_TTL_EXTEND_TO: u32 = 518_400;
const ENTRY_TTL_THRESHOLD: u32 = 518_400;
const ENTRY_TTL_EXTEND_TO: u32 = 1_555_200;
const MAX_NAME_LEN: u32 = 128;
const MAX_INTERVAL: u64 = 315_360_000; // ten years
const MAX_AUTH_WINDOW: u64 = 315_360_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    InvalidConfig = 5,
    PlanNotFound = 6,
    PlanInactive = 7,
    SubscriptionNotFound = 8,
    AlreadySubscribed = 9,
    SubscriptionCancelled = 10,
    NotDue = 11,
    AuthorizationNotStarted = 12,
    AuthorizationExpired = 13,
    CycleLimitReached = 14,
    InsufficientAllowance = 15,
    InsufficientBalance = 16,
    Overflow = 17,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub id: u64,
    pub merchant: Address,
    pub name: String,
    pub token: Address,
    pub amount: i128,
    pub interval: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionStatus {
    Active,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authorization {
    /// First timestamp at which a keeper may pull payment.
    pub starts_at: u64,
    /// Inclusive final timestamp at which a payment may be pulled.
    pub ends_at: u64,
    /// Lifetime upper bound, including charges already collected.
    pub max_cycles: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub id: u64,
    pub plan_id: u64,
    pub subscriber: Address,
    pub next_charge_at: u64,
    pub ends_at: u64,
    pub max_cycles: u32,
    pub cycles_charged: u32,
    pub total_charged: i128,
    pub status: SubscriptionStatus,
}

#[contracttype]
enum InstanceKey {
    Admin,
    Paused,
    PlanCounter,
    SubscriptionCounter,
}

#[contracttype]
enum DataKey {
    Plan(u64),
    Subscription(u64),
    SubscriberPlan(Address, u64),
}

#[contract]
pub struct SubscriptionBilling;

fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

fn require_initialized(env: &Env) -> Result<Address, Error> {
    let admin = env
        .storage()
        .instance()
        .get(&InstanceKey::Admin)
        .ok_or(Error::NotInitialized)?;
    bump_instance(env);
    Ok(admin)
}

fn require_running(env: &Env) -> Result<(), Error> {
    require_initialized(env)?;
    if env
        .storage()
        .instance()
        .get(&InstanceKey::Paused)
        .unwrap_or(false)
    {
        return Err(Error::Paused);
    }
    Ok(())
}

fn get_plan(env: &Env, id: u64) -> Result<Plan, Error> {
    let key = DataKey::Plan(id);
    let plan = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::PlanNotFound)?;
    env.storage()
        .persistent()
        .extend_ttl(&key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
    Ok(plan)
}

fn set_plan(env: &Env, plan: &Plan) {
    let key = DataKey::Plan(plan.id);
    env.storage().persistent().set(&key, plan);
    env.storage()
        .persistent()
        .extend_ttl(&key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
}

fn get_subscription(env: &Env, id: u64) -> Result<Subscription, Error> {
    let key = DataKey::Subscription(id);
    let subscription = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::SubscriptionNotFound)?;
    env.storage()
        .persistent()
        .extend_ttl(&key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
    let index_key = DataKey::SubscriberPlan(subscription.subscriber.clone(), subscription.plan_id);
    if env.storage().persistent().get::<_, u64>(&index_key) == Some(id) {
        env.storage()
            .persistent()
            .extend_ttl(&index_key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
    }
    Ok(subscription)
}

fn set_subscription(env: &Env, subscription: &Subscription) {
    let key = DataKey::Subscription(subscription.id);
    env.storage().persistent().set(&key, subscription);
    env.storage()
        .persistent()
        .extend_ttl(&key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
}

fn next_id(env: &Env, key: InstanceKey) -> Result<u64, Error> {
    let id: u64 = env.storage().instance().get(&key).unwrap_or(1);
    env.storage()
        .instance()
        .set(&key, &id.checked_add(1).ok_or(Error::Overflow)?);
    Ok(id)
}

#[contractimpl]
impl SubscriptionBilling {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&InstanceKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&InstanceKey::Admin, &admin);
        env.storage().instance().set(&InstanceKey::Paused, &false);
        bump_instance(&env);
        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    /// Emergency pause. Cancellation and read methods deliberately remain live.
    pub fn set_paused(env: Env, paused: bool) -> Result<(), Error> {
        let admin = require_initialized(&env)?;
        admin.require_auth();
        env.storage().instance().set(&InstanceKey::Paused, &paused);
        env.events().publish((symbol_short!("paused"),), paused);
        Ok(())
    }

    /// Creates an immutable billing plan owned by `merchant`.
    pub fn create_plan(
        env: Env,
        merchant: Address,
        name: String,
        token: Address,
        amount: i128,
        interval: u64,
    ) -> Result<u64, Error> {
        require_running(&env)?;
        merchant.require_auth();
        if name.len() == 0
            || name.len() > MAX_NAME_LEN
            || amount <= 0
            || interval == 0
            || interval > MAX_INTERVAL
        {
            return Err(Error::InvalidConfig);
        }
        let id = next_id(&env, InstanceKey::PlanCounter)?;
        set_plan(
            &env,
            &Plan {
                id,
                merchant: merchant.clone(),
                name,
                token,
                amount,
                interval,
                active: true,
            },
        );
        env.events().publish((symbol_short!("plan"), merchant), id);
        Ok(id)
    }

    pub fn set_plan_active(env: Env, plan_id: u64, active: bool) -> Result<(), Error> {
        require_running(&env)?;
        let mut plan = get_plan(&env, plan_id)?;
        plan.merchant.require_auth();
        plan.active = active;
        set_plan(&env, &plan);
        env.events()
            .publish((symbol_short!("plan_act"), plan_id), active);
        Ok(())
    }

    /// Records bounded consent after the subscriber has approved this contract
    /// as a spender on `plan.token`. No payment is pulled by this call.
    pub fn subscribe(
        env: Env,
        subscriber: Address,
        plan_id: u64,
        authorization: Authorization,
    ) -> Result<u64, Error> {
        require_running(&env)?;
        subscriber.require_auth();
        let plan = get_plan(&env, plan_id)?;
        if !plan.active {
            return Err(Error::PlanInactive);
        }
        let now = env.ledger().timestamp();
        if authorization.max_cycles == 0
            || authorization.ends_at < authorization.starts_at
            || authorization.ends_at < now
            || authorization.ends_at - now > MAX_AUTH_WINDOW
        {
            return Err(Error::InvalidConfig);
        }
        plan.amount
            .checked_mul(authorization.max_cycles as i128)
            .ok_or(Error::Overflow)?;

        let index_key = DataKey::SubscriberPlan(subscriber.clone(), plan_id);
        if let Some(existing_id) = env.storage().persistent().get::<_, u64>(&index_key) {
            let existing = get_subscription(&env, existing_id)?;
            let terminal = existing.status == SubscriptionStatus::Cancelled
                || now > existing.ends_at
                || existing.cycles_charged >= existing.max_cycles;
            if !terminal {
                return Err(Error::AlreadySubscribed);
            }
        }

        let vault = env.current_contract_address();
        if token::Client::new(&env, &plan.token).allowance(&subscriber, &vault) < plan.amount {
            return Err(Error::InsufficientAllowance);
        }
        let id = next_id(&env, InstanceKey::SubscriptionCounter)?;
        let next_charge_at = if authorization.starts_at < now {
            now
        } else {
            authorization.starts_at
        };
        let subscription = Subscription {
            id,
            plan_id,
            subscriber: subscriber.clone(),
            next_charge_at,
            ends_at: authorization.ends_at,
            max_cycles: authorization.max_cycles,
            cycles_charged: 0,
            total_charged: 0,
            status: SubscriptionStatus::Active,
        };
        set_subscription(&env, &subscription);
        env.storage().persistent().set(&index_key, &id);
        env.storage()
            .persistent()
            .extend_ttl(&index_key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
        env.events().publish(
            (symbol_short!("subscribe"), subscriber),
            (id, plan_id, authorization.ends_at, authorization.max_cycles),
        );
        Ok(id)
    }

    /// Pulls exactly one cycle. Scheduling from `now` prevents a keeper from
    /// draining several missed periods in a burst.
    pub fn charge(env: Env, subscription_id: u64) -> Result<i128, Error> {
        require_running(&env)?;
        let mut subscription = get_subscription(&env, subscription_id)?;
        if subscription.status == SubscriptionStatus::Cancelled {
            return Err(Error::SubscriptionCancelled);
        }
        let index_key =
            DataKey::SubscriberPlan(subscription.subscriber.clone(), subscription.plan_id);
        if env.storage().persistent().get::<_, u64>(&index_key) != Some(subscription_id) {
            return Err(Error::AlreadySubscribed);
        }
        let now = env.ledger().timestamp();
        if now < subscription.next_charge_at {
            return if subscription.cycles_charged == 0 {
                Err(Error::AuthorizationNotStarted)
            } else {
                Err(Error::NotDue)
            };
        }
        if now > subscription.ends_at {
            return Err(Error::AuthorizationExpired);
        }
        if subscription.cycles_charged >= subscription.max_cycles {
            return Err(Error::CycleLimitReached);
        }
        let plan = get_plan(&env, subscription.plan_id)?;
        let vault = env.current_contract_address();
        let token_client = token::Client::new(&env, &plan.token);
        if token_client.allowance(&subscription.subscriber, &vault) < plan.amount {
            return Err(Error::InsufficientAllowance);
        }
        if token_client.balance(&subscription.subscriber) < plan.amount {
            return Err(Error::InsufficientBalance);
        }

        subscription.cycles_charged = subscription
            .cycles_charged
            .checked_add(1)
            .ok_or(Error::Overflow)?;
        subscription.total_charged = subscription
            .total_charged
            .checked_add(plan.amount)
            .ok_or(Error::Overflow)?;
        subscription.next_charge_at = now.checked_add(plan.interval).ok_or(Error::Overflow)?;
        set_subscription(&env, &subscription);

        token_client.transfer_from(
            &vault,
            &subscription.subscriber,
            &plan.merchant,
            &plan.amount,
        );
        env.events().publish(
            (symbol_short!("charge"), subscription.subscriber),
            (subscription_id, plan.merchant, plan.amount),
        );
        Ok(plan.amount)
    }

    /// Subscriber consent can be renewed or reduced only with fresh subscriber
    /// authorization. It never revives a cancelled subscription.
    pub fn update_authorization(
        env: Env,
        subscription_id: u64,
        ends_at: u64,
        max_cycles: u32,
    ) -> Result<(), Error> {
        require_running(&env)?;
        let mut subscription = get_subscription(&env, subscription_id)?;
        subscription.subscriber.require_auth();
        if subscription.status == SubscriptionStatus::Cancelled {
            return Err(Error::SubscriptionCancelled);
        }
        let index_key =
            DataKey::SubscriberPlan(subscription.subscriber.clone(), subscription.plan_id);
        if env.storage().persistent().get::<_, u64>(&index_key) != Some(subscription_id) {
            return Err(Error::AlreadySubscribed);
        }
        let now = env.ledger().timestamp();
        if ends_at < now
            || ends_at - now > MAX_AUTH_WINDOW
            || max_cycles < subscription.cycles_charged
        {
            return Err(Error::InvalidConfig);
        }
        let plan = get_plan(&env, subscription.plan_id)?;
        plan.amount
            .checked_mul(max_cycles as i128)
            .ok_or(Error::Overflow)?;
        subscription.ends_at = ends_at;
        subscription.max_cycles = max_cycles;
        set_subscription(&env, &subscription);
        env.events().publish(
            (symbol_short!("auth_upd"), subscription.subscriber),
            (subscription_id, ends_at, max_cycles),
        );
        Ok(())
    }

    /// Subscriber cancellation is always available, including while paused.
    pub fn cancel(env: Env, caller: Address, subscription_id: u64) -> Result<(), Error> {
        require_initialized(&env)?;
        caller.require_auth();
        let mut subscription = get_subscription(&env, subscription_id)?;
        let plan = get_plan(&env, subscription.plan_id)?;
        if caller != subscription.subscriber && caller != plan.merchant {
            return Err(Error::Unauthorized);
        }
        if subscription.status == SubscriptionStatus::Cancelled {
            return Err(Error::SubscriptionCancelled);
        }
        subscription.status = SubscriptionStatus::Cancelled;
        set_subscription(&env, &subscription);
        let index_key =
            DataKey::SubscriberPlan(subscription.subscriber.clone(), subscription.plan_id);
        if env.storage().persistent().get::<_, u64>(&index_key) == Some(subscription_id) {
            env.storage().persistent().remove(&index_key);
        }
        env.events().publish(
            (symbol_short!("cancel"), subscription.subscriber),
            subscription_id,
        );
        Ok(())
    }

    pub fn get_plan(env: Env, plan_id: u64) -> Result<Plan, Error> {
        require_initialized(&env)?;
        get_plan(&env, plan_id)
    }

    pub fn get_subscription(env: Env, subscription_id: u64) -> Result<Subscription, Error> {
        require_initialized(&env)?;
        get_subscription(&env, subscription_id)
    }

    pub fn get_subscription_id(env: Env, subscriber: Address, plan_id: u64) -> Option<u64> {
        let key = DataKey::SubscriberPlan(subscriber, plan_id);
        let id = env.storage().persistent().get(&key);
        if id.is_some() {
            env.storage()
                .persistent()
                .extend_ttl(&key, ENTRY_TTL_THRESHOLD, ENTRY_TTL_EXTEND_TO);
        }
        id
    }

    pub fn is_charge_due(env: Env, subscription_id: u64) -> Result<bool, Error> {
        require_initialized(&env)?;
        let subscription = get_subscription(&env, subscription_id)?;
        let now = env.ledger().timestamp();
        Ok(subscription.status == SubscriptionStatus::Active
            && now >= subscription.next_charge_at
            && now <= subscription.ends_at
            && subscription.cycles_charged < subscription.max_cycles)
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&InstanceKey::Paused)
            .unwrap_or(false)
    }
}
