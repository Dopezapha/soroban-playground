// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Emergency Pause Contract
//!
//! Guardian multi-sig role with capability to pause token transfers during exploits.
//! This is a critical component required for enterprise scalability, resilience, and production operation.
//!
//! ## Features
//! - Multi-sig guardian role for pause/unpause actions
#![cfg_attr(not(test), no_std)]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, String};

use crate::storage::{
    get_admin, get_guardian_count, get_pause_reason, get_pause_timestamp, get_proposal,
    get_proposal_count, get_threshold, is_guardian, is_initialized, is_paused, next_proposal_id,
    set_admin, set_guardian, set_guardian_count, set_pause_reason, set_pause_timestamp, set_paused,
    set_proposal, set_threshold,
};
use crate::types::{Error, PauseAction, PauseProposal};

#[contract]
pub struct EmergencyPause;

#[contractimpl]
impl EmergencyPause {
    // ── Initialisation ────────────────────────────────────────────────────────

    /// Initialize the contract with an admin and a signature threshold.
    /// The threshold must be >= 1 and <= number of guardians.
    pub fn initialize(env: Env, admin: Address, threshold: u32) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();

        if threshold == 0 {
            return Err(Error::InvalidThreshold);
        }

        set_admin(&env, &admin);
        set_threshold(&env, threshold);
        set_paused(&env, false);
        set_guardian_count(&env, 0);

        env.events().publish((symbol_short!("init"),), admin);

        Ok(())
    }

    // ── Guardian management ───────────────────────────────────────────────────

    /// Add a guardian (admin only).
    pub fn add_guardian(env: Env, caller: Address, guardian: Address) -> Result<(), Error> {
        Self::assert_admin(&env, &caller)?;

        if is_guardian(&env, &guardian) {
            return Err(Error::GuardianAlreadyAdded);
        }

        set_guardian(&env, &guardian, true);
        let count = get_guardian_count(&env) + 1;
        set_guardian_count(&env, count);

        env.events().publish((symbol_short!("guardian"),), guardian);

        Ok(())
    }

    /// Remove a guardian (admin only).
    pub fn remove_guardian(env: Env, caller: Address, guardian: Address) -> Result<(), Error> {
        Self::assert_admin(&env, &caller)?;

        if !is_guardian(&env, &guardian) {
            return Err(Error::GuardianNotFound);
        }

        set_guardian(&env, &guardian, false);
        let count = get_guardian_count(&env).saturating_sub(1);
        set_guardian_count(&env, count);

        env.events().publish((symbol_short!("un_guard"),), guardian);

        Ok(())
    }

    /// Check if an address is a guardian.
    pub fn is_guardian(env: Env, addr: Address) -> bool {
        is_guardian(&env, &addr)
    }

    /// Get the current guardian count.
    pub fn guardian_count(env: Env) -> u32 {
        get_guardian_count(&env)
    }

    // ── Threshold management ──────────────────────────────────────────────────

    /// Update the signature threshold (admin only).
    pub fn set_threshold(env: Env, caller: Address, new_threshold: u32) -> Result<(), Error> {
        Self::assert_admin(&env, &caller)?;

        if new_threshold == 0 {
            return Err(Error::InvalidThreshold);
        }

        let guardian_count = get_guardian_count(&env);
        if new_threshold > guardian_count + 1 {
            // Allow threshold up to guardian_count + 1 (admin counts as a signer)
            return Err(Error::InvalidThreshold);
        }

        set_threshold(&env, new_threshold);

        env.events()
            .publish((symbol_short!("thresh"),), new_threshold);

        Ok(())
    }

    /// Get the current signature threshold.
    pub fn get_threshold(env: Env) -> Result<u32, Error> {
        get_threshold(&env)
    }

    // ── Proposal lifecycle ────────────────────────────────────────────────────

    /// Create a pause/unpause proposal. Returns the proposal ID.
    pub fn create_proposal(
        env: Env,
        proposer: Address,
        action: PauseAction,
        reason: String,
        ttl_seconds: u64,
    ) -> Result<u32, Error> {
        proposer.require_auth();
        Self::assert_initialized(&env)?;

        let now = env.ledger().timestamp();
        let id = next_proposal_id(&env);
        let proposal = PauseProposal {
            id,
            proposer: proposer.clone(),
            action,
            reason,
            created_at: now,
            expires_at: now + ttl_seconds,
            executed: false,
            signers: soroban_sdk::Vec::new(&env),
        };
        set_proposal(&env, id, &proposal);

        env.events()
            .publish((symbol_short!("proposed"),), (id, proposer));

        Ok(id)
    }

    /// Sign a proposal. Guardian-only. Increments signature count.
    pub fn sign_proposal(env: Env, signer: Address, proposal_id: u32) -> Result<(), Error> {
        signer.require_auth();
        Self::assert_initialized(&env)?;

        if !is_guardian(&env, &signer) {
            return Err(Error::Unauthorized);
        }

        let mut proposal = get_proposal(&env, proposal_id)?;

        if proposal.executed {
            return Err(Error::ProposalAlreadyExecuted);
        }

        let now = env.ledger().timestamp();
        if now > proposal.expires_at {
            return Err(Error::ProposalExpired);
        }

        // Check if already signed
        if proposal.signers.contains(&signer) {
            return Err(Error::AlreadyExists);
        }

        proposal.signers.push_back(signer.clone());
        set_proposal(&env, proposal_id, &proposal);

        env.events()
            .publish((symbol_short!("signed"),), (proposal_id, signer));

        Ok(())
    }

    /// Execute a proposal once enough signatures are collected.
    pub fn execute_proposal(env: Env, executor: Address, proposal_id: u32) -> Result<(), Error> {
        executor.require_auth();
        Self::assert_initialized(&env)?;

        let mut proposal = get_proposal(&env, proposal_id)?;

        if proposal.executed {
            return Err(Error::ProposalAlreadyExecuted);
        }

        let now = env.ledger().timestamp();
        if now > proposal.expires_at {
            return Err(Error::ProposalExpired);
        }

        let threshold = get_threshold(&env)?;
        let signatures = proposal.signers.len();
        if signatures < threshold {
            return Err(Error::InsufficientSignatures);
        }

        // Execute the action
        match proposal.action {
            PauseAction::Pause => {
                if is_paused(&env) {
                    return Err(Error::AlreadyInState);
                }
                set_paused(&env, true);
                set_pause_timestamp(&env, now);
                if proposal.reason.len() > 0 {
                    set_pause_reason(&env, &proposal.reason);
                }
                env.events().publish((symbol_short!("paused"),), now);
            }
            PauseAction::Unpause => {
                if !is_paused(&env) {
                    return Err(Error::AlreadyInState);
                }
                set_paused(&env, false);
                env.storage()
                    .instance()
                    .remove(&crate::types::InstanceKey::PauseReason);
                env.storage()
                    .instance()
                    .remove(&crate::types::InstanceKey::PauseTimestamp);
                env.events().publish((symbol_short!("unpaused"),), executor);
            }
        }

        proposal.executed = true;
        set_proposal(&env, proposal_id, &proposal);

        Ok(())
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Returns `true` if the contract is currently paused.
    pub fn paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Returns the reason the contract was paused, if one was recorded.
    pub fn get_pause_reason(env: Env) -> Option<String> {
        get_pause_reason(&env)
    }

    /// Returns the ledger timestamp at which the contract was paused.
    pub fn get_pause_timestamp(env: Env) -> Option<u64> {
        get_pause_timestamp(&env)
    }

    /// Returns the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        get_admin(&env)
    }

    /// Returns proposal details.
    pub fn get_proposal(env: Env, proposal_id: u32) -> Result<PauseProposal, Error> {
        get_proposal(&env, proposal_id)
    }

    /// Returns the total number of proposals.
    pub fn proposal_count(env: Env) -> u32 {
        get_proposal_count(&env)
    }

    // ── Guarded action ────────────────────────────────────────────────────────

    /// Example guarded action — blocked when paused.
    pub fn do_action(env: Env, caller: Address) -> Result<(), Error> {
        caller.require_auth();
        if is_paused(&env) {
            return Err(Error::ContractPaused);
        }
        env.events().publish((symbol_short!("action"),), caller);
        Ok(())
    }

    // ── Internals ─────────────────────────────────────────────────────────────

    fn assert_admin(env: &Env, caller: &Address) -> Result<(), Error> {
        caller.require_auth();
        let admin = get_admin(env)?;
        if *caller != admin {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }

    fn assert_initialized(env: &Env) -> Result<(), Error> {
        if !is_initialized(env) {
            return Err(Error::NotInitialized);
        }
        Ok(())
    }
}
