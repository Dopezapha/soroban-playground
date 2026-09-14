// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

use soroban_sdk::{contracterror, contracttype, Address, String};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    AlreadyExists = 4,
    InvalidStatus = 5,
    EmptyField = 6,
    InsufficientBalance = 7,
    MeterNotFound = 8,
    TradeNotFound = 9,
    InvalidTradeAmount = 10,
    SelfTrade = 11,
    TradeAlreadySettled = 12,
    InvalidMeterReading = 13,
    MeterNotRegistered = 14,
}

// ── Enums ─────────────────────────────────────────────────────────────────────

/// Status of a trade order.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TradeStatus {
    Open,
    Matched,
    Settled,
    Cancelled,
}

/// Type of energy being traded.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum EnergyType {
    Solar,
    Wind,
    Hydro,
    Battery,
    Grid,
}

/// Status of a smart meter.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeterStatus {
    Active,
    Inactive,
    Suspended,
}

// ── Structs ───────────────────────────────────────────────────────────────────

/// A smart meter IoT device registered in the system.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmartMeter {
    pub id: u32,
    pub owner: Address,
    pub location: String,
    pub energy_type: EnergyType,
    pub capacity_kw: i128,
    pub status: MeterStatus,
    pub registered_at: u64,
    pub last_reading: u64,
    pub total_generated: i128,
}

/// A reading from a smart meter (proof of energy generation/consumption).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeterReading {
    pub meter_id: u32,
    pub timestamp: u64,
    pub kwh_generated: i128,
    pub kwh_consumed: i128,
    pub proof_hash: u64,
    pub verified: bool,
}

/// A trade order for energy.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TradeOrder {
    pub id: u32,
    pub seller_meter_id: u32,
    pub buyer: Option<Address>,
    pub energy_type: EnergyType,
    pub kwh_amount: i128,
    pub price_per_kwh: i128,
    pub total_price: i128,
    pub status: TradeStatus,
    pub created_at: u64,
    pub settled_at: Option<u64>,
}

/// A peer-to-peer energy trade between two parties.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnergyTrade {
    pub id: u32,
    pub seller: Address,
    pub buyer: Address,
    pub seller_meter_id: u32,
    pub buyer_meter_id: u32,
    pub kwh_amount: i128,
    pub price_per_kwh: i128,
    pub total_price: i128,
    pub energy_type: EnergyType,
    pub status: TradeStatus,
    pub created_at: u64,
    pub settled_at: Option<u64>,
}

/// Energy balance for a participant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnergyBalance {
    pub address: Address,
    pub kwh_balance: i128,
    pub total_earned: i128,
    pub total_spent: i128,
}

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum InstanceKey {
    Admin,
    MeterCount,
    TradeCount,
    TotalEnergyTraded,
}

#[contracttype]
pub enum DataKey {
    Meter(u32),
    MeterReading(u32, u64), // meter_id, timestamp
    Trade(u32),
    Balance(Address),
    MeterOwner(Address), // address -> list of meter IDs (simplified as count)
}
