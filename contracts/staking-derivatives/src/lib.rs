// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Liquid Staking Derivative (LSD) Exchange Rate Accrual Engine
//!
//! Implements liquid staking derivatives (lstTokens) with:
//! - Continuous exchange rate accrual based on validator rewards
//! - Unbonding queue management with configurable unbonding period
//! - Validator reward accounting and reward distribution
//! - lstToken mint/burn on stake/unstake
//! - Emergency pause mechanism
//! - Per-validator stake tracking
//!
//! ## Exchange Rate Model
//!
//! The exchange rate (underlying per lstToken) increases over time as rewards accrue:
//!
//!   rate_new = rate_old * (1 + reward_rate_bps/10000 * elapsed_seconds / SECONDS_PER_YEAR)
//!
//! Users always stake/unstake using the current rate, so early stakers
//! benefit from compounding as the rate increases.
//!
//! ## Unbonding Queue
//!
//! Unstaking does not return tokens immediately. Instead a `UnbondEntry` is created
//! with a `release_ts = now + unbonding_period`. The user calls `claim_unbonded`
//! after the period to receive their underlying tokens.

#![no_std]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

use crate::storage::{
    get_admin, get_exchange_rate, get_last_accrual_ts, get_lst_balance, get_reward_rate_bps,
    get_total_lst, get_total_rewards, get_total_staked, get_total_unbonding, get_unbond_count,
    get_unbond_entry, get_unbonding_period, get_validator_stake, is_initialized, is_paused,
    set_admin, set_exchange_rate, set_last_accrual_ts, set_lst_balance, set_paused,
    set_reward_rate_bps, set_total_lst, set_total_rewards, set_total_staked, set_total_unbonding,
    set_unbond_count, set_unbond_entry, set_unbonding_period, set_validator_stake,
};
use crate::types::{Error, ProtocolMetrics, UnbondEntry, UserInfo};

/// Exchange rate precision: 1 lstToken = RATE_PRECISION underlying at genesis.
const RATE_PRECISION: i128 = 1_000_000;
/// Seconds per year (365.25 days).
const SECONDS_PER_YEAR: i128 = 31_557_600;

#[contract]
pub struct StakingDerivatives;

#[contractimpl]
impl StakingDerivatives {
    // ── Initialization ────────────────────────────────────────────────────────

    /// Initialize the staking derivatives protocol.
    ///
    /// - `reward_rate_bps`: Annual reward rate in basis points (e.g. 500 = 5% APY).
    /// - `unbonding_period`: Seconds to wait before unbonded tokens can be claimed.
    pub fn initialize(
        env: Env,
        admin: Address,
        reward_rate_bps: i128,
        unbonding_period: u64,
    ) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        if reward_rate_bps <= 0 || reward_rate_bps > 10_000 {
            return Err(Error::InvalidRate);
        }
        if unbonding_period == 0 {
            return Err(Error::InvalidPeriod);
        }
        set_admin(&env, &admin);
        set_reward_rate_bps(&env, reward_rate_bps);
        set_unbonding_period(&env, unbonding_period);
        set_exchange_rate(&env, RATE_PRECISION); // 1:1 at genesis
        set_last_accrual_ts(&env, env.ledger().timestamp());
        Ok(())
    }

    // ── Staking ───────────────────────────────────────────────────────────────

    /// Stake `amount` underlying tokens.
    ///
    /// Accrues rewards first to update exchange rate, then mints lstTokens:
    ///   lst_minted = amount * RATE_PRECISION / exchange_rate
    ///
    /// Returns the number of lstTokens minted.
    pub fn stake(env: Env, staker: Address, amount: i128) -> Result<i128, Error> {
        ensure_active(&env)?;
        staker.require_auth();
        if amount <= 0 {
            return Err(Error::ZeroAmount);
        }

        // Accrue rewards before computing the rate.
        accrue_rewards(&env)?;

        let rate = get_exchange_rate(&env);
        // lst_minted = amount * RATE_PRECISION / rate
        let lst_minted = amount.checked_mul(RATE_PRECISION).ok_or(Error::Overflow)? / rate;

        if lst_minted == 0 {
            return Err(Error::ZeroAmount);
        }

        set_lst_balance(&env, &staker, get_lst_balance(&env, &staker) + lst_minted);
        set_total_lst(&env, get_total_lst(&env) + lst_minted);
        set_total_staked(&env, get_total_staked(&env) + amount);

        env.events()
            .publish((symbol_short!("staked"),), (staker, amount, lst_minted));
        Ok(lst_minted)
    }

    /// Unstake `lst_amount` lstTokens, creating an unbonding queue entry.
    ///
    /// Accrues rewards first, then:
    ///   underlying = lst_amount * exchange_rate / RATE_PRECISION
    ///
    /// Returns the underlying amount queued for unbonding and the release timestamp.
    pub fn unstake(env: Env, staker: Address, lst_amount: i128) -> Result<(i128, u64), Error> {
        ensure_active(&env)?;
        staker.require_auth();
        if lst_amount <= 0 {
            return Err(Error::ZeroAmount);
        }

        let lst_bal = get_lst_balance(&env, &staker);
        if lst_bal < lst_amount {
            return Err(Error::InsufficientBalance);
        }

        // Accrue rewards before redeeming.
        accrue_rewards(&env)?;

        let rate = get_exchange_rate(&env);
        let underlying = lst_amount.checked_mul(rate).ok_or(Error::Overflow)? / RATE_PRECISION;

        // Burn lstTokens.
        set_lst_balance(&env, &staker, lst_bal - lst_amount);
        set_total_lst(&env, get_total_lst(&env) - lst_amount);
        set_total_staked(&env, (get_total_staked(&env) - underlying).max(0));

        // Create unbonding entry.
        let release_ts = env.ledger().timestamp() + get_unbonding_period(&env);
        let idx = get_unbond_count(&env, &staker);
        let entry = UnbondEntry {
            amount: underlying,
            release_ts,
            claimed: false,
        };
        set_unbond_entry(&env, &staker, idx, &entry);
        set_unbond_count(&env, &staker, idx + 1);
        set_total_unbonding(&env, get_total_unbonding(&env) + underlying);

        env.events().publish(
            (symbol_short!("unstaked"),),
            (staker, lst_amount, underlying, release_ts),
        );
        Ok((underlying, release_ts))
    }

    /// Claim an unbonded entry after its release timestamp has passed.
    ///
    /// Returns the amount of underlying tokens released.
    pub fn claim_unbonded(env: Env, staker: Address, entry_idx: u32) -> Result<i128, Error> {
        ensure_active(&env)?;
        staker.require_auth();

        let entry = get_unbond_entry(&env, &staker, entry_idx).ok_or(Error::InvalidEntry)?;

        if entry.claimed {
            return Err(Error::AlreadyClaimed);
        }

        let now = env.ledger().timestamp();
        if now < entry.release_ts {
            return Err(Error::UnbondNotReady);
        }

        // Mark as claimed.
        let claimed_entry = UnbondEntry {
            amount: entry.amount,
            release_ts: entry.release_ts,
            claimed: true,
        };
        set_unbond_entry(&env, &staker, entry_idx, &claimed_entry);
        set_total_unbonding(&env, (get_total_unbonding(&env) - entry.amount).max(0));

        env.events()
            .publish((symbol_short!("claimed"),), (staker, entry.amount));
        Ok(entry.amount)
    }

    // ── Validator accounting ──────────────────────────────────────────────────

    /// Allocate staking to a specific validator.
    ///
    /// Admin-only. Records how much of the total pool is staked with each validator.
    pub fn delegate_to_validator(
        env: Env,
        admin: Address,
        validator: Address,
        amount: i128,
    ) -> Result<(), Error> {
        ensure_active(&env)?;
        admin.require_auth();
        require_admin(&env, &admin)?;
        if amount < 0 {
            return Err(Error::ZeroAmount);
        }

        let current = get_validator_stake(&env, &validator);
        set_validator_stake(&env, &validator, current + amount);

        env.events()
            .publish((symbol_short!("delegated"),), (validator, amount));
        Ok(())
    }

    /// Report rewards from a validator (admin only).
    ///
    /// This increases total_staked (rewards are compounded into the pool),
    /// which naturally raises the exchange rate on next accrual.
    pub fn report_validator_rewards(
        env: Env,
        admin: Address,
        validator: Address,
        reward_amount: i128,
    ) -> Result<(), Error> {
        ensure_active(&env)?;
        admin.require_auth();
        require_admin(&env, &admin)?;
        if reward_amount <= 0 {
            return Err(Error::ZeroAmount);
        }

        // Add rewards to pool (compound into staked balance).
        set_total_staked(&env, get_total_staked(&env) + reward_amount);
        set_total_rewards(&env, get_total_rewards(&env) + reward_amount);

        // Immediately recompute exchange rate from actual pool state.
        recompute_rate_from_pool(&env)?;

        env.events()
            .publish((symbol_short!("reward"),), (validator, reward_amount));
        Ok(())
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Update annual reward rate (basis points). Admin only.
    pub fn set_reward_rate(env: Env, admin: Address, new_rate_bps: i128) -> Result<(), Error> {
        admin.require_auth();
        require_admin(&env, &admin)?;
        if new_rate_bps <= 0 || new_rate_bps > 10_000 {
            return Err(Error::InvalidRate);
        }
        // Accrue first with old rate.
        accrue_rewards(&env)?;
        set_reward_rate_bps(&env, new_rate_bps);
        env.events()
            .publish((symbol_short!("ratechg"),), new_rate_bps);
        Ok(())
    }

    /// Pause / unpause the protocol. Admin only.
    pub fn set_paused(env: Env, admin: Address, paused: bool) -> Result<(), Error> {
        admin.require_auth();
        require_admin(&env, &admin)?;
        set_paused(&env, paused);
        let sym = if paused {
            symbol_short!("paused")
        } else {
            symbol_short!("unpaused")
        };
        env.events().publish((sym,), ());
        Ok(())
    }

    // ── Reward accrual ────────────────────────────────────────────────────────

    /// Manually trigger reward accrual. Anyone can call this.
    ///
    /// Updates the exchange rate based on elapsed time and reward_rate_bps.
    pub fn accrue(env: Env) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        accrue_rewards(&env)?;
        Ok(get_exchange_rate(&env))
    }

    // ── Read-only ─────────────────────────────────────────────────────────────

    /// Current exchange rate (RATE_PRECISION units per lstToken).
    pub fn get_exchange_rate(env: Env) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        Ok(get_exchange_rate(&env))
    }

    /// Preview how many lstTokens `amount` underlying would mint at current rate.
    pub fn preview_stake(env: Env, amount: i128) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        let rate = get_exchange_rate(&env);
        Ok(amount.checked_mul(RATE_PRECISION).ok_or(Error::Overflow)? / rate)
    }

    /// Preview how much underlying `lst_amount` lstTokens would redeem at current rate.
    pub fn preview_unstake(env: Env, lst_amount: i128) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        let rate = get_exchange_rate(&env);
        Ok(lst_amount.checked_mul(rate).ok_or(Error::Overflow)? / RATE_PRECISION)
    }

    /// Get user position.
    pub fn get_user_info(env: Env, staker: Address) -> Result<UserInfo, Error> {
        ensure_initialized(&env)?;
        let lst_bal = get_lst_balance(&env, &staker);
        let rate = get_exchange_rate(&env);
        let underlying_value = lst_bal.checked_mul(rate).ok_or(Error::Overflow)? / RATE_PRECISION;
        Ok(UserInfo {
            lst_balance: lst_bal,
            underlying_value,
            pending_unbond_count: get_unbond_count(&env, &staker),
        })
    }

    /// Get unbonding entry details.
    pub fn get_unbond_entry(env: Env, staker: Address, idx: u32) -> Result<UnbondEntry, Error> {
        ensure_initialized(&env)?;
        get_unbond_entry(&env, &staker, idx).ok_or(Error::InvalidEntry)
    }

    /// Get protocol-level metrics.
    pub fn get_metrics(env: Env) -> Result<ProtocolMetrics, Error> {
        ensure_initialized(&env)?;
        Ok(ProtocolMetrics {
            total_staked: get_total_staked(&env),
            total_lst: get_total_lst(&env),
            exchange_rate: get_exchange_rate(&env),
            total_rewards: get_total_rewards(&env),
            total_unbonding: get_total_unbonding(&env),
            last_accrual_ts: get_last_accrual_ts(&env),
        })
    }

    /// Get validator stake allocation.
    pub fn get_validator_stake(env: Env, validator: Address) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        Ok(get_validator_stake(&env, &validator))
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn ensure_initialized(env: &Env) -> Result<(), Error> {
    if !is_initialized(env) {
        return Err(Error::NotInitialized);
    }
    Ok(())
}

fn ensure_active(env: &Env) -> Result<(), Error> {
    ensure_initialized(env)?;
    if is_paused(env) {
        return Err(Error::Paused);
    }
    Ok(())
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
    let admin = get_admin(env)?;
    if &admin != caller {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

/// Accrue time-based rewards and update exchange rate using elapsed seconds.
///
/// rate_new = rate_old + rate_old * reward_rate_bps * elapsed / (10000 * SECONDS_PER_YEAR)
///
/// This is a linear approximation of continuous compounding suitable for
/// on-chain computation.
fn accrue_rewards(env: &Env) -> Result<(), Error> {
    let now = env.ledger().timestamp();
    let last = get_last_accrual_ts(env);
    if now <= last {
        return Ok(());
    }
    let elapsed = (now - last) as i128;
    let rate = get_exchange_rate(env);
    let reward_bps = get_reward_rate_bps(env);

    // delta_rate = rate * reward_bps * elapsed / (10000 * SECONDS_PER_YEAR)
    let delta_rate = rate
        .checked_mul(reward_bps)
        .ok_or(Error::Overflow)?
        .checked_mul(elapsed)
        .ok_or(Error::Overflow)?
        / (10_000 * SECONDS_PER_YEAR);

    set_exchange_rate(env, rate + delta_rate);
    set_last_accrual_ts(env, now);
    Ok(())
}

/// Recompute exchange rate from actual pool balances (used after reward injection).
///
/// rate = total_staked * RATE_PRECISION / total_lst
fn recompute_rate_from_pool(env: &Env) -> Result<(), Error> {
    let total_lst = get_total_lst(env);
    if total_lst == 0 {
        return Ok(()); // No lstTokens in circulation yet.
    }
    let total_staked = get_total_staked(env);
    let new_rate = total_staked
        .checked_mul(RATE_PRECISION)
        .ok_or(Error::Overflow)?
        / total_lst;
    set_exchange_rate(env, new_rate.max(RATE_PRECISION)); // Rate can only go up.
    Ok(())
}
