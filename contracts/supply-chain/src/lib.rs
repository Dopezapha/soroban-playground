// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Supply Chain Tracking Contract
//!
//! Tracks products from registration through delivery with:
//! - Provenance verification via metadata hashes
//! - Checkpoint-based traceability (location + handler at each step)
//! - Quality assurance reports by authorised inspectors
//! - Recall mechanism for compromised products
//! - Cold-chain temperature logging with SLA penalty enforcement
#![no_std]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, Address, Env, String};

use crate::storage::{
    get_admin, get_checkpoint, get_checkpoint_count, get_cold_chain_sla, get_penalty_count,
    get_penalty_record, get_product, get_product_count, get_quality_report, get_temperature_log,
    is_handler, is_initialized, is_inspector, next_penalty_id, next_sla_id, set_admin,
    set_checkpoint, set_checkpoint_count, set_cold_chain_sla, set_handler, set_inspector,
    set_penalty_count, set_penalty_record, set_product, set_product_count, set_quality_report,
    set_temperature_log,
};
use crate::types::{
    Checkpoint, ColdChainSla, Error, PenaltyRecord, Product, ProductStatus, QualityReport,
    QualityResult, SlaStatus, TemperatureLog, TemperatureLogStatus,
};

#[contract]
pub struct SupplyChain;

#[contractimpl]
impl SupplyChain {
    // ── Initialisation ────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        set_admin(&env, &admin);
        Ok(())
    }

    // ── Role management ───────────────────────────────────────────────────────

    pub fn add_inspector(env: Env, caller: Address, inspector: Address) -> Result<(), Error> {
        caller.require_auth();
        let admin = get_admin(&env)?;
        if caller != admin {
            return Err(Error::Unauthorized);
        }
        set_inspector(&env, &inspector, true);
        Ok(())
    }

    pub fn remove_inspector(env: Env, caller: Address, inspector: Address) -> Result<(), Error> {
        caller.require_auth();
        if caller != get_admin(&env)? {
            return Err(Error::Unauthorized);
        }
        set_inspector(&env, &inspector, false);
        Ok(())
    }

    pub fn add_handler(env: Env, caller: Address, handler: Address) -> Result<(), Error> {
        caller.require_auth();
        if caller != get_admin(&env)? {
            return Err(Error::Unauthorized);
        }
        set_handler(&env, &handler, true);
        Ok(())
    }

    pub fn remove_handler(env: Env, caller: Address, handler: Address) -> Result<(), Error> {
        caller.require_auth();
        if caller != get_admin(&env)? {
            return Err(Error::Unauthorized);
        }
        set_handler(&env, &handler, false);
        Ok(())
    }

    // ── Product registration ──────────────────────────────────────────────────

    /// Register a new product. Returns the new product ID.
    pub fn register_product(
        env: Env,
        owner: Address,
        name: String,
        metadata_hash: u64,
    ) -> Result<u32, Error> {
        owner.require_auth();
        if name.is_empty() {
            return Err(Error::EmptyName);
        }
        let id = get_product_count(&env) + 1;
        let now = env.ledger().timestamp();
        let product = Product {
            id,
            owner,
            name,
            metadata_hash,
            status: ProductStatus::Registered,
            created_at: now,
            updated_at: now,
        };
        set_product(&env, &product);
        set_product_count(&env, id);
        Ok(id)
    }

    // ── Checkpoint / traceability ─────────────────────────────────────────────

    /// Record a supply chain checkpoint (location + handler).
    pub fn add_checkpoint(
        env: Env,
        handler: Address,
        product_id: u32,
        location_hash: u64,
        notes_hash: u64,
    ) -> Result<u32, Error> {
        handler.require_auth();
        if !is_handler(&env, &handler) {
            return Err(Error::NotHandler);
        }
        let mut product = get_product(&env, product_id)?;
        if product.status == ProductStatus::Recalled {
            return Err(Error::AlreadyRecalled);
        }

        let index = get_checkpoint_count(&env, product_id) + 1;
        let now = env.ledger().timestamp();
        let checkpoint = Checkpoint {
            product_id,
            index,
            handler,
            location_hash,
            notes_hash,
            timestamp: now,
        };
        set_checkpoint(&env, &checkpoint);
        set_checkpoint_count(&env, product_id, index);

        product.status = ProductStatus::InTransit;
        product.updated_at = now;
        set_product(&env, &product);
        Ok(index)
    }

    /// Update product status (e.g. AtWarehouse, QualityCheck, Delivered).
    pub fn update_status(
        env: Env,
        caller: Address,
        product_id: u32,
        new_status: ProductStatus,
    ) -> Result<(), Error> {
        caller.require_auth();
        let admin = get_admin(&env)?;
        let is_auth = caller == admin || is_handler(&env, &caller);
        if !is_auth {
            return Err(Error::Unauthorized);
        }
        let mut product = get_product(&env, product_id)?;
        if product.status == ProductStatus::Recalled {
            return Err(Error::AlreadyRecalled);
        }
        product.status = new_status;
        product.updated_at = env.ledger().timestamp();
        set_product(&env, &product);
        Ok(())
    }

    // ── Quality assurance ─────────────────────────────────────────────────────

    /// Submit a quality inspection report.
    pub fn submit_quality_report(
        env: Env,
        inspector: Address,
        product_id: u32,
        result: QualityResult,
        report_hash: u64,
    ) -> Result<(), Error> {
        inspector.require_auth();
        if !is_inspector(&env, &inspector) {
            return Err(Error::NotInspector);
        }
        let mut product = get_product(&env, product_id)?;
        if product.status == ProductStatus::Recalled {
            return Err(Error::AlreadyRecalled);
        }
        let now = env.ledger().timestamp();
        let report = QualityReport {
            product_id,
            inspector,
            result,
            report_hash,
            timestamp: now,
        };
        set_quality_report(&env, &report);

        product.status = match result {
            QualityResult::Pass => ProductStatus::Approved,
            QualityResult::Fail => ProductStatus::Rejected,
            QualityResult::Pending => ProductStatus::QualityCheck,
        };
        product.updated_at = now;
        set_product(&env, &product);
        Ok(())
    }

    // ── Recall ────────────────────────────────────────────────────────────────

    pub fn recall_product(env: Env, caller: Address, product_id: u32) -> Result<(), Error> {
        caller.require_auth();
        if caller != get_admin(&env)? {
            return Err(Error::Unauthorized);
        }
        let mut product = get_product(&env, product_id)?;
        if product.status == ProductStatus::Recalled {
            return Err(Error::AlreadyRecalled);
        }
        product.status = ProductStatus::Recalled;
        product.updated_at = env.ledger().timestamp();
        set_product(&env, &product);
        Ok(())
    }

    // ── Cold Chain SLA ────────────────────────────────────────────────────────

    /// Create a cold-chain SLA for a product. Returns the SLA ID.
    pub fn create_cold_chain_sla(
        env: Env,
        caller: Address,
        product_id: u32,
        min_temp_celsius: i32,
        max_temp_celsius: i32,
        max_violation_minutes: u32,
        penalty_per_violation: i128,
        deposit_amount: i128,
        duration_seconds: u64,
    ) -> Result<u32, Error> {
        caller.require_auth();

        if min_temp_celsius >= max_temp_celsius {
            return Err(Error::InvalidTemperatureRange);
        }

        let now = env.ledger().timestamp();
        let id = next_sla_id(&env);
        let sla = ColdChainSla {
            id,
            product_id,
            min_temp_celsius,
            max_temp_celsius,
            max_violation_minutes,
            penalty_per_violation,
            deposit_amount,
            status: SlaStatus::Active,
            created_at: now,
            expires_at: now + duration_seconds,
            violation_count: 0,
            total_penalties: 0,
        };
        set_cold_chain_sla(&env, id, &sla);

        env.events()
            .publish((soroban_sdk::symbol_short!("sla"),), (id, product_id));

        Ok(id)
    }

    /// Log a temperature reading for a product.
    pub fn log_temperature(
        env: Env,
        recorder: Address,
        product_id: u32,
        temperature_celsius: i32,
        humidity_percent: u32,
        proof_hash: u64,
    ) -> Result<(), Error> {
        recorder.require_auth();

        let now = env.ledger().timestamp();
        let status = Self::check_temperature_status(&env, product_id, temperature_celsius);

        let log = TemperatureLog {
            product_id,
            timestamp: now,
            temperature_celsius,
            humidity_percent,
            status: status.clone(),
            recorded_by: recorder,
            proof_hash,
        };
        set_temperature_log(&env, product_id, now, &log);

        // Check for SLA violations
        if status == TemperatureLogStatus::Violation {
            Self::record_violation(&env, product_id, now)?;
        }

        env.events()
            .publish((soroban_sdk::symbol_short!("temp"),), (product_id, now));

        Ok(())
    }

    /// Record a temperature violation and apply penalties.
    fn record_violation(env: &Env, product_id: u32, timestamp: u64) -> Result<(), Error> {
        // Find active SLA for this product
        let sla_count = storage::get_sla_count(env);
        for i in 1..=sla_count {
            if let Ok(mut sla) = get_cold_chain_sla(env, i) {
                if sla.product_id == product_id && sla.status == SlaStatus::Active {
                    let now = env.ledger().timestamp();
                    if now > sla.expires_at {
                        sla.status = SlaStatus::Expired;
                        set_cold_chain_sla(env, i, &sla);
                        continue;
                    }

                    // Record penalty
                    let penalty_id = next_penalty_id(env);
                    let penalty = PenaltyRecord {
                        id: penalty_id,
                        sla_id: i,
                        product_id,
                        violation_timestamp: timestamp,
                        duration_minutes: 1, // Simplified - in production would calculate
                        penalty_amount: sla.penalty_per_violation,
                        recorded_by: get_admin(env)?,
                    };
                    set_penalty_record(env, penalty_id, &penalty);

                    // Update SLA
                    sla.violation_count += 1;
                    sla.total_penalties += sla.penalty_per_violation;
                    set_cold_chain_sla(env, i, &sla);

                    // Update product status
                    let mut product = get_product(env, product_id)?;
                    product.status = ProductStatus::TemperatureViolation;
                    product.updated_at = now;
                    set_product(env, &product);

                    break;
                }
            }
        }
        Ok(())
    }

    /// Check temperature status against active SLA.
    fn check_temperature_status(
        env: &Env,
        product_id: u32,
        temperature_celsius: i32,
    ) -> TemperatureLogStatus {
        let sla_count = storage::get_sla_count(env);
        for i in 1..=sla_count {
            if let Ok(sla) = get_cold_chain_sla(env, i) {
                if sla.product_id == product_id && sla.status == SlaStatus::Active {
                    let now = env.ledger().timestamp();
                    if now > sla.expires_at {
                        continue;
                    }

                    if temperature_celsius < sla.min_temp_celsius
                        || temperature_celsius > sla.max_temp_celsius
                    {
                        return TemperatureLogStatus::Violation;
                    }

                    // Warning if within buffer of limit (only when range is wide enough)
                    let range = sla.max_temp_celsius - sla.min_temp_celsius;
                    let buffer = if range >= 15 { 5 } else { range / 6 };
                    if buffer > 0
                        && (temperature_celsius < sla.min_temp_celsius + buffer
                            || temperature_celsius > sla.max_temp_celsius - buffer)
                    {
                        return TemperatureLogStatus::Warning;
                    }

                    return TemperatureLogStatus::Normal;
                }
            }
        }
        TemperatureLogStatus::Normal
    }

    /// Get temperature log for a product at a specific timestamp.
    pub fn get_temperature_log(
        env: Env,
        product_id: u32,
        timestamp: u64,
    ) -> Option<TemperatureLog> {
        get_temperature_log(&env, product_id, timestamp)
    }

    /// Get cold-chain SLA details.
    pub fn get_cold_chain_sla(env: Env, sla_id: u32) -> Result<ColdChainSla, Error> {
        get_cold_chain_sla(&env, sla_id)
    }

    /// Get penalty record details.
    pub fn get_penalty_record(env: Env, penalty_id: u32) -> Result<PenaltyRecord, Error> {
        get_penalty_record(&env, penalty_id)
    }

    /// Get total number of penalties.
    pub fn get_penalty_count(env: Env) -> u32 {
        storage::get_penalty_count(&env)
    }

    /// Get total number of SLAs.
    pub fn get_sla_count(env: Env) -> u32 {
        storage::get_sla_count(&env)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_product(env: Env, product_id: u32) -> Result<Product, Error> {
        get_product(&env, product_id)
    }

    pub fn get_checkpoint(env: Env, product_id: u32, index: u32) -> Result<Checkpoint, Error> {
        get_checkpoint(&env, product_id, index).ok_or(Error::ProductNotFound)
    }

    pub fn get_checkpoint_count(env: Env, product_id: u32) -> u32 {
        get_checkpoint_count(&env, product_id)
    }

    pub fn get_quality_report(env: Env, product_id: u32) -> Result<QualityReport, Error> {
        get_quality_report(&env, product_id)
    }

    pub fn product_count(env: Env) -> u32 {
        get_product_count(&env)
    }
}
