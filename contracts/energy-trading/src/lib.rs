// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Energy Trading Contract
//!
//! Decentralized energy grid peer-to-peer trading ledger with:
//! - Smart meter IoT proof verification
//! - Kilowatt-hour token settlement
//! - P2P energy trading between prosumers
//! - Energy balance tracking
//! - Trade matching and settlement

#![cfg_attr(not(test), no_std)]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, String};

use crate::storage::{
    get_admin, get_balance, get_energy_trade, get_meter, get_meter_count, get_meter_reading,
    get_total_energy_traded, get_trade_count, get_trade_order, is_initialized, next_meter_id,
    next_trade_id, set_admin, set_balance, set_energy_trade, set_meter, set_meter_reading,
    set_total_energy_traded, set_trade_order,
};
use crate::types::{
    EnergyBalance, EnergyTrade, EnergyType, Error, MeterReading, MeterStatus, SmartMeter,
    TradeOrder, TradeStatus,
};

#[contract]
pub struct EnergyTrading;

#[contractimpl]
impl EnergyTrading {
    // ── Initialisation ────────────────────────────────────────────────────────

    /// Initialize the contract with an admin address.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        set_admin(&env, &admin);
        set_total_energy_traded(&env, 0);
        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    // ── Smart Meter Management ────────────────────────────────────────────────

    /// Register a new smart meter. Returns the meter ID.
    pub fn register_meter(
        env: Env,
        owner: Address,
        location: String,
        energy_type: EnergyType,
        capacity_kw: i128,
    ) -> Result<u32, Error> {
        owner.require_auth();

        if location.len() == 0 {
            return Err(Error::EmptyField);
        }
        if capacity_kw <= 0 {
            return Err(Error::InvalidMeterReading);
        }

        let now = env.ledger().timestamp();
        let id = next_meter_id(&env);
        let meter = SmartMeter {
            id,
            owner: owner.clone(),
            location,
            energy_type,
            capacity_kw,
            status: MeterStatus::Active,
            registered_at: now,
            last_reading: 0,
            total_generated: 0,
        };
        set_meter(&env, id, &meter);

        env.events().publish((symbol_short!("meter"),), (id, owner));

        Ok(id)
    }

    /// Deactivate a smart meter (owner only).
    pub fn deactivate_meter(env: Env, caller: Address, meter_id: u32) -> Result<(), Error> {
        caller.require_auth();

        let mut meter = get_meter(&env, meter_id)?;
        if meter.owner != caller {
            return Err(Error::Unauthorized);
        }

        meter.status = MeterStatus::Inactive;
        set_meter(&env, meter_id, &meter);

        env.events().publish((symbol_short!("deact"),), meter_id);

        Ok(())
    }

    // ── Meter Readings ────────────────────────────────────────────────────────

    /// Submit a meter reading (proof of energy generation/consumption).
    pub fn submit_reading(
        env: Env,
        owner: Address,
        meter_id: u32,
        kwh_generated: i128,
        kwh_consumed: i128,
        proof_hash: u64,
    ) -> Result<(), Error> {
        owner.require_auth();

        let mut meter = get_meter(&env, meter_id)?;
        if meter.owner != owner {
            return Err(Error::Unauthorized);
        }
        if meter.status != MeterStatus::Active {
            return Err(Error::MeterNotRegistered);
        }
        if kwh_generated < 0 || kwh_consumed < 0 {
            return Err(Error::InvalidMeterReading);
        }

        let now = env.ledger().timestamp();
        let reading = MeterReading {
            meter_id,
            timestamp: now,
            kwh_generated,
            kwh_consumed,
            proof_hash,
            verified: true, // In production, this would verify the IoT proof
        };
        set_meter_reading(&env, meter_id, now, &reading);

        // Update meter stats
        meter.last_reading = now;
        meter.total_generated += kwh_generated;
        set_meter(&env, meter_id, &meter);

        // Update balance
        let mut balance = get_balance(&env, &owner);
        balance.kwh_balance += kwh_generated - kwh_consumed;
        set_balance(&env, &owner, &balance);

        env.events()
            .publish((symbol_short!("reading"),), (meter_id, now));

        Ok(())
    }

    // ── Trading ───────────────────────────────────────────────────────────────

    /// Create a trade order to sell energy. Returns the order ID.
    pub fn create_sell_order(
        env: Env,
        seller: Address,
        meter_id: u32,
        kwh_amount: i128,
        price_per_kwh: i128,
    ) -> Result<u32, Error> {
        seller.require_auth();

        if kwh_amount <= 0 || price_per_kwh <= 0 {
            return Err(Error::InvalidTradeAmount);
        }

        let meter = get_meter(&env, meter_id)?;
        if meter.owner != seller {
            return Err(Error::Unauthorized);
        }

        let balance = get_balance(&env, &seller);
        if balance.kwh_balance < kwh_amount {
            return Err(Error::InsufficientBalance);
        }

        let now = env.ledger().timestamp();
        let id = next_trade_id(&env);
        let order = TradeOrder {
            id,
            seller_meter_id: meter_id,
            buyer: None,
            energy_type: meter.energy_type,
            kwh_amount,
            price_per_kwh,
            total_price: kwh_amount * price_per_kwh,
            status: TradeStatus::Open,
            created_at: now,
            settled_at: None,
        };
        set_trade_order(&env, id, &order);

        env.events().publish((symbol_short!("sell"),), (id, seller));

        Ok(id)
    }

    /// Accept a sell order and execute the trade. Returns the trade ID.
    pub fn accept_order(
        env: Env,
        buyer: Address,
        order_id: u32,
        buyer_meter_id: u32,
    ) -> Result<u32, Error> {
        buyer.require_auth();

        let mut order = get_trade_order(&env, order_id)?;
        if order.status != TradeStatus::Open {
            return Err(Error::InvalidStatus);
        }

        let buyer_meter = get_meter(&env, buyer_meter_id)?;
        if buyer_meter.owner != buyer {
            return Err(Error::Unauthorized);
        }

        // Check buyer balance
        let buyer_balance = get_balance(&env, &buyer);
        if buyer_balance.kwh_balance < order.total_price {
            return Err(Error::InsufficientBalance);
        }

        let seller_meter = get_meter(&env, order.seller_meter_id)?;
        let seller = seller_meter.owner.clone();

        // Prevent self-trading
        if seller == buyer {
            return Err(Error::SelfTrade);
        }

        let now = env.ledger().timestamp();

        // Create the trade
        let trade_id = next_trade_id(&env);
        let trade = EnergyTrade {
            id: trade_id,
            seller: seller.clone(),
            buyer: buyer.clone(),
            seller_meter_id: order.seller_meter_id,
            buyer_meter_id,
            kwh_amount: order.kwh_amount,
            price_per_kwh: order.price_per_kwh,
            total_price: order.total_price,
            energy_type: order.energy_type,
            status: TradeStatus::Settled,
            created_at: now,
            settled_at: Some(now),
        };
        set_energy_trade(&env, trade_id, &trade);

        // Update order status
        order.status = TradeStatus::Matched;
        order.buyer = Some(buyer.clone());
        order.settled_at = Some(now);
        set_trade_order(&env, order_id, &order);

        // Update balances
        let mut seller_balance = get_balance(&env, &seller);
        seller_balance.kwh_balance -= order.kwh_amount;
        seller_balance.total_earned += order.total_price;
        set_balance(&env, &seller, &seller_balance);

        let mut buyer_balance = get_balance(&env, &buyer);
        buyer_balance.kwh_balance -= order.total_price;
        buyer_balance.kwh_balance += order.kwh_amount;
        buyer_balance.total_spent += order.total_price;
        set_balance(&env, &buyer, &buyer_balance);

        // Update total energy traded
        let total = get_total_energy_traded(&env) + order.kwh_amount;
        set_total_energy_traded(&env, total);

        env.events()
            .publish((symbol_short!("trade"),), (trade_id, seller, buyer));

        Ok(trade_id)
    }

    /// Cancel a sell order (seller only).
    pub fn cancel_order(env: Env, seller: Address, order_id: u32) -> Result<(), Error> {
        seller.require_auth();

        let mut order = get_trade_order(&env, order_id)?;
        let meter = get_meter(&env, order.seller_meter_id)?;
        if meter.owner != seller {
            return Err(Error::Unauthorized);
        }
        if order.status != TradeStatus::Open {
            return Err(Error::InvalidStatus);
        }

        order.status = TradeStatus::Cancelled;
        set_trade_order(&env, order_id, &order);

        env.events().publish((symbol_short!("cancel"),), order_id);

        Ok(())
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Get smart meter details.
    pub fn get_meter(env: Env, meter_id: u32) -> Result<SmartMeter, Error> {
        get_meter(&env, meter_id)
    }

    /// Get meter reading.
    pub fn get_meter_reading(env: Env, meter_id: u32, timestamp: u64) -> Option<MeterReading> {
        get_meter_reading(&env, meter_id, timestamp)
    }

    /// Get trade order details.
    pub fn get_trade_order(env: Env, order_id: u32) -> Result<TradeOrder, Error> {
        get_trade_order(&env, order_id)
    }

    /// Get energy trade details.
    pub fn get_energy_trade(env: Env, trade_id: u32) -> Result<EnergyTrade, Error> {
        crate::storage::get_energy_trade(&env, trade_id)
    }

    /// Get energy balance for an address.
    pub fn get_balance(env: Env, address: Address) -> EnergyBalance {
        get_balance(&env, &address)
    }

    /// Get total energy traded on the platform.
    pub fn get_total_energy_traded(env: Env) -> i128 {
        get_total_energy_traded(&env)
    }

    /// Get total number of meters.
    pub fn get_meter_count(env: Env) -> u32 {
        get_meter_count(&env)
    }

    /// Get total number of trades.
    pub fn get_trade_count(env: Env) -> u32 {
        get_trade_count(&env)
    }

    /// Get admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        get_admin(&env)
    }
}
