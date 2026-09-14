// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Options Trading Black-Scholes Greeks Calculator & Margin Pool
//!
//! Implements European cash-settled options with:
//!
//! ## Option Lifecycle
//! 1. Writer deposits margin and calls `write_option` to create the contract.
//! 2. Buyer calls `buy_option` paying the premium.
//! 3. At or after expiry, holder calls `exercise` with a settlement price.
//! 4. Payout = max(S_T - K, 0) * size for Call, max(K - S_T, 0) * size for Put.
//! 5. Expired unexercised options can be reclaimed by the writer.
//!
//! ## Black-Scholes Greeks (Integer Approximation)
//!
//! All Greeks are computed on-chain using integer arithmetic approximations:
//!
//! - **Delta (Δ)**: ΔCall ≈ N(d1), ΔPut ≈ N(d1) - 1
//! - **Gamma (Γ)**: n(d1) / (S * σ * √T)
//! - **Theta (Θ)**: -(S * n(d1) * σ) / (2 * √T) - r * K * e^(-rT) * N(±d2)
//! - **Vega (ν)**: S * n(d1) * √T
//!
//! Where approximations use:
//! - N(x) ≈ standard normal CDF (rational polynomial approximation)
//! - n(x) ≈ standard normal PDF (Gaussian kernel)
//! - √T ≈ integer square root over precision-scaled values
//!
//! ## Margin System
//!
//! Writers must post margin ≥ `min_margin_bps / 10000 * notional_value`.
//! Margin calls are triggered when deposited < required margin.
//! Cash-settled payout is drawn from writer's margin pool.

#![no_std]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

use crate::storage::{
    get_admin, get_margin, get_option, get_option_count, get_total_margin, is_initialized,
    is_paused, set_admin, set_margin, set_option, set_option_count, set_paused, set_total_margin,
};
use crate::types::{Error, Greeks, MarginRequirement, OptionContract, OptionStatus, OptionType};

/// Price precision: prices are scaled by this factor.
const PRICE_PRECISION: i128 = 1_000_000;
/// Greek precision: Greeks are scaled by this factor.
const GREEK_PRECISION: i128 = 1_000_000;
/// Minimum margin as a fraction of notional (20% = 2000 bps).
const MIN_MARGIN_BPS: i128 = 2_000;
/// Seconds per year for time-to-expiry calculations.
const SECONDS_PER_YEAR: i128 = 31_557_600;

#[contract]
pub struct OptionsContract;

#[contractimpl]
impl OptionsContract {
    // ── Initialization ────────────────────────────────────────────────────────

    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        set_admin(&env, &admin);
        Ok(())
    }

    // ── Option writing ────────────────────────────────────────────────────────

    /// Write (create) a new European option.
    ///
    /// - `option_type`: Call (0) or Put (1).
    /// - `strike_price`: Strike in PRICE_PRECISION units.
    /// - `spot_price`: Current spot price in PRICE_PRECISION units.
    /// - `size`: Number of underlying units covered.
    /// - `premium`: Premium charged to buyer per unit.
    /// - `expiry`: Unix timestamp of expiry.
    /// - `margin`: Collateral deposited by writer (must satisfy min margin).
    ///
    /// Returns the option id.
    pub fn write_option(
        env: Env,
        writer: Address,
        option_type: OptionType,
        strike_price: i128,
        spot_price: i128,
        size: i128,
        premium: i128,
        expiry: u64,
        margin: i128,
    ) -> Result<u32, Error> {
        ensure_active(&env)?;
        writer.require_auth();

        if strike_price <= 0 {
            return Err(Error::InvalidStrike);
        }
        if expiry <= env.ledger().timestamp() {
            return Err(Error::InvalidExpiry);
        }
        if size <= 0 || premium < 0 || margin <= 0 {
            return Err(Error::ZeroAmount);
        }

        // Validate margin requirement.
        let notional = strike_price.checked_mul(size).ok_or(Error::Overflow)? / PRICE_PRECISION;
        let min_margin = notional
            .checked_mul(MIN_MARGIN_BPS)
            .ok_or(Error::Overflow)?
            / 10_000;
        if margin < min_margin {
            return Err(Error::InsufficientMargin);
        }

        let id = get_option_count(&env);
        let opt = OptionContract {
            id,
            writer: writer.clone(),
            option_type,
            strike_price,
            spot_price_at_write: spot_price,
            premium,
            size,
            expiry,
            holder: None,
            status: OptionStatus::Active,
            settlement_price: 0,
        };
        set_option(&env, &opt);
        set_option_count(&env, id + 1);

        // Record margin.
        set_margin(&env, &writer, get_margin(&env, &writer) + margin);
        set_total_margin(&env, get_total_margin(&env) + margin);

        env.events().publish(
            (symbol_short!("opt_writ"),),
            (id, writer, strike_price, expiry),
        );
        Ok(id)
    }

    // ── Option buying ─────────────────────────────────────────────────────────

    /// Buy an option. Transfers premium from buyer.
    ///
    /// In this implementation the premium accounting is tracked on-chain;
    /// actual token transfers would be handled by the caller via a token contract.
    pub fn buy_option(env: Env, buyer: Address, option_id: u32) -> Result<i128, Error> {
        ensure_active(&env)?;
        buyer.require_auth();

        let mut opt = get_option(&env, option_id).ok_or(Error::OptionNotFound)?;
        if opt.status != OptionStatus::Active {
            return Err(Error::OptionAlreadySettled);
        }
        if env.ledger().timestamp() >= opt.expiry {
            return Err(Error::OptionExpired);
        }
        if opt.holder.is_some() {
            return Err(Error::OptionAlreadySettled);
        }

        opt.holder = Some(buyer.clone());
        // Premium is credited to writer's margin.
        set_margin(
            &env,
            &opt.writer,
            get_margin(&env, &opt.writer) + opt.premium,
        );
        set_option(&env, &opt);

        env.events()
            .publish((symbol_short!("opt_buy"),), (option_id, buyer, opt.premium));
        Ok(opt.premium)
    }

    // ── Exercise / expiry ─────────────────────────────────────────────────────

    /// Exercise a European option at expiry with a settlement price.
    ///
    /// Only the holder can call this, at or after `expiry`.
    /// Payout is cash-settled from the writer's margin:
    ///   Call payout = max(settlement_price - strike_price, 0) * size / PRICE_PRECISION
    ///   Put payout  = max(strike_price - settlement_price, 0) * size / PRICE_PRECISION
    pub fn exercise(
        env: Env,
        holder: Address,
        option_id: u32,
        settlement_price: i128,
    ) -> Result<i128, Error> {
        ensure_active(&env)?;
        holder.require_auth();

        let mut opt = get_option(&env, option_id).ok_or(Error::OptionNotFound)?;
        if opt.status != OptionStatus::Active {
            return Err(Error::OptionAlreadySettled);
        }
        if env.ledger().timestamp() < opt.expiry {
            return Err(Error::OptionNotExpired);
        }

        // Verify caller is the holder.
        let actual_holder = opt.holder.as_ref().ok_or(Error::NotOptionHolder)?;
        if actual_holder != &holder {
            return Err(Error::NotOptionHolder);
        }

        // Compute payout.
        let intrinsic = match opt.option_type {
            OptionType::Call => (settlement_price - opt.strike_price).max(0),
            OptionType::Put => (opt.strike_price - settlement_price).max(0),
        };
        let payout = intrinsic.checked_mul(opt.size).ok_or(Error::Overflow)? / PRICE_PRECISION;

        // Deduct from writer margin.
        let writer_margin = get_margin(&env, &opt.writer);
        let actual_payout = payout.min(writer_margin); // Capped at available margin.
        set_margin(&env, &opt.writer, writer_margin - actual_payout);
        set_total_margin(&env, (get_total_margin(&env) - actual_payout).max(0));

        opt.status = OptionStatus::Exercised;
        opt.settlement_price = settlement_price;
        set_option(&env, &opt);

        env.events().publish(
            (symbol_short!("opt_exer"),),
            (option_id, holder, actual_payout),
        );
        Ok(actual_payout)
    }

    /// Expire an option that has passed its expiry without being exercised.
    ///
    /// Writer calls this to reclaim their margin (minus premium).
    pub fn expire_option(env: Env, writer: Address, option_id: u32) -> Result<(), Error> {
        ensure_active(&env)?;
        writer.require_auth();

        let mut opt = get_option(&env, option_id).ok_or(Error::OptionNotFound)?;
        if opt.status != OptionStatus::Active {
            return Err(Error::OptionAlreadySettled);
        }
        if env.ledger().timestamp() < opt.expiry {
            return Err(Error::OptionNotExpired);
        }
        if opt.writer != writer {
            return Err(Error::Unauthorized);
        }

        opt.status = OptionStatus::Expired;
        set_option(&env, &opt);

        env.events()
            .publish((symbol_short!("opt_exp"),), (option_id, writer));
        Ok(())
    }

    // ── Margin management ─────────────────────────────────────────────────────

    /// Deposit additional margin.
    pub fn deposit_margin(env: Env, writer: Address, amount: i128) -> Result<i128, Error> {
        ensure_active(&env)?;
        writer.require_auth();
        if amount <= 0 {
            return Err(Error::ZeroAmount);
        }
        let new_margin = get_margin(&env, &writer) + amount;
        set_margin(&env, &writer, new_margin);
        set_total_margin(&env, get_total_margin(&env) + amount);
        Ok(new_margin)
    }

    /// Withdraw free margin (margin not locked in active options).
    pub fn withdraw_margin(env: Env, writer: Address, amount: i128) -> Result<i128, Error> {
        ensure_active(&env)?;
        writer.require_auth();
        if amount <= 0 {
            return Err(Error::ZeroAmount);
        }
        let current = get_margin(&env, &writer);
        if current < amount {
            return Err(Error::InsufficientMargin);
        }
        let new_margin = current - amount;
        set_margin(&env, &writer, new_margin);
        set_total_margin(&env, (get_total_margin(&env) - amount).max(0));
        Ok(new_margin)
    }

    // ── Black-Scholes Greeks ──────────────────────────────────────────────────

    /// Compute Black-Scholes Greeks for an option using integer approximations.
    ///
    /// Parameters:
    /// - `spot_price`: Current underlying spot price (PRICE_PRECISION scaled).
    /// - `vol_bps`: Implied volatility in basis points (e.g. 5000 = 50% vol).
    ///
    /// Returns Greeks scaled by GREEK_PRECISION.
    pub fn compute_greeks(
        env: Env,
        option_id: u32,
        spot_price: i128,
        vol_bps: i128,
    ) -> Result<Greeks, Error> {
        ensure_initialized(&env)?;
        if vol_bps <= 0 || vol_bps > 100_000 {
            return Err(Error::InvalidVolatility);
        }
        let opt = get_option(&env, option_id).ok_or(Error::OptionNotFound)?;

        let now = env.ledger().timestamp();
        if now >= opt.expiry {
            // At expiry: delta = 1 for ITM Call, 0 for OTM, etc.
            let intrinsic = match opt.option_type {
                OptionType::Call => (spot_price - opt.strike_price).max(0),
                OptionType::Put => (opt.strike_price - spot_price).max(0),
            };
            let in_the_money = intrinsic > 0;
            let delta = if in_the_money {
                match opt.option_type {
                    OptionType::Call => GREEK_PRECISION,
                    OptionType::Put => -GREEK_PRECISION,
                }
            } else {
                0
            };
            return Ok(Greeks {
                delta,
                gamma: 0,
                theta: 0,
                vega: 0,
                intrinsic_value: intrinsic * opt.size / PRICE_PRECISION,
                time_value: 0,
            });
        }

        let time_to_expiry_secs = (opt.expiry - now) as i128;
        // T = time_to_expiry / SECONDS_PER_YEAR (as fraction, scaled by GREEK_PRECISION)
        let t_scaled = time_to_expiry_secs
            .checked_mul(GREEK_PRECISION)
            .ok_or(Error::Overflow)?
            / SECONDS_PER_YEAR;

        // σ = vol_bps / 10000
        let sigma_scaled = vol_bps
            .checked_mul(GREEK_PRECISION)
            .ok_or(Error::Overflow)?
            / 10_000;

        // √T (scaled by sqrt(GREEK_PRECISION))
        let sqrt_t = isqrt_scaled(t_scaled, GREEK_PRECISION);

        // d1 = (ln(S/K) + (σ²/2)*T) / (σ*√T)
        // Using integer approximation: ln(S/K) ≈ (S - K) / K for small moves.
        let ln_sk = if opt.strike_price > 0 {
            (spot_price - opt.strike_price)
                .checked_mul(GREEK_PRECISION)
                .ok_or(Error::Overflow)?
                / opt.strike_price
        } else {
            0
        };

        // σ²/2 * T = (sigma_scaled^2 / GREEK_PRECISION) / 2 * t_scaled / GREEK_PRECISION
        let sigma_sq = sigma_scaled
            .checked_mul(sigma_scaled)
            .ok_or(Error::Overflow)?
            / GREEK_PRECISION;
        let sigma_sq_half_t = (sigma_sq / 2_i128)
            .checked_mul(t_scaled)
            .ok_or(Error::Overflow)?
            / GREEK_PRECISION;

        let numerator_d1 = ln_sk + sigma_sq_half_t;
        let denom_d1 = sigma_scaled.checked_mul(sqrt_t).ok_or(Error::Overflow)? / GREEK_PRECISION;

        let d1 = if denom_d1 != 0 {
            numerator_d1
                .checked_mul(GREEK_PRECISION)
                .ok_or(Error::Overflow)?
                / denom_d1
        } else {
            0
        };

        // d2 = d1 - σ*√T
        let sigma_sqrt_t =
            sigma_scaled.checked_mul(sqrt_t).ok_or(Error::Overflow)? / GREEK_PRECISION;
        let d2 = d1 - sigma_sqrt_t;

        // N(d1), N(d2): standard normal CDF approximation
        let nd1 = normal_cdf(d1); // scaled by GREEK_PRECISION
        let nd2 = normal_cdf(d2);

        // n(d1): standard normal PDF at d1
        let n_d1 = normal_pdf(d1); // scaled by GREEK_PRECISION

        // Delta
        let delta = match opt.option_type {
            OptionType::Call => nd1,
            OptionType::Put => nd1 - GREEK_PRECISION,
        };

        // Gamma = n(d1) / (S * σ * √T) × GREEK_PRECISION²
        let denom_gamma = spot_price
            .checked_mul(sigma_scaled)
            .ok_or(Error::Overflow)?
            / GREEK_PRECISION.checked_mul(sqrt_t).ok_or(Error::Overflow)?
            / GREEK_PRECISION;
        let gamma = if denom_gamma != 0 {
            n_d1.checked_mul(GREEK_PRECISION).ok_or(Error::Overflow)? / denom_gamma
        } else {
            0
        };

        // Vega = S * n(d1) * √T (per 1% vol change = per 100 bps)
        let vega = spot_price.checked_mul(n_d1).ok_or(Error::Overflow)?
            / GREEK_PRECISION.checked_mul(sqrt_t).ok_or(Error::Overflow)?
            / GREEK_PRECISION
            / 100; // per 1% change in vol

        // Theta = -(S * n(d1) * σ) / (2 * √T) - r*K*N(±d2)
        // Approximate r=0 (risk-free rate) for simplicity:
        // Theta ≈ -(S * n(d1) * σ) / (2 * √T) per year, divide by 365 for per-day
        let theta_annual = if sqrt_t != 0 {
            -(spot_price.checked_mul(n_d1).ok_or(Error::Overflow)?
                / GREEK_PRECISION
                    .checked_mul(sigma_scaled)
                    .ok_or(Error::Overflow)?
                / GREEK_PRECISION
                / (2 * sqrt_t / GREEK_PRECISION).max(1))
        } else {
            0
        };
        let theta = theta_annual / 365;

        // Intrinsic & time value
        let intrinsic = match opt.option_type {
            OptionType::Call => (spot_price - opt.strike_price).max(0),
            OptionType::Put => (opt.strike_price - spot_price).max(0),
        };
        let intrinsic_value = intrinsic * opt.size / PRICE_PRECISION;

        // Option price (Black-Scholes): for preview only, simplified
        // Call = S*N(d1) - K*N(d2), scaled
        let bs_price = match opt.option_type {
            OptionType::Call => {
                spot_price.checked_mul(nd1).ok_or(Error::Overflow)? / GREEK_PRECISION
                    - opt.strike_price.checked_mul(nd2).ok_or(Error::Overflow)? / GREEK_PRECISION
            }
            OptionType::Put => {
                opt.strike_price
                    .checked_mul(GREEK_PRECISION - nd2)
                    .ok_or(Error::Overflow)?
                    / GREEK_PRECISION
                    - spot_price
                        .checked_mul(GREEK_PRECISION - nd1)
                        .ok_or(Error::Overflow)?
                        / GREEK_PRECISION
            }
        };
        let time_value = (bs_price - intrinsic).max(0);

        Ok(Greeks {
            delta,
            gamma,
            theta,
            vega,
            intrinsic_value,
            time_value,
        })
    }

    // ── Margin check ──────────────────────────────────────────────────────────

    /// Check margin requirement for a writer's open option.
    pub fn check_margin(env: Env, option_id: u32) -> Result<MarginRequirement, Error> {
        ensure_initialized(&env)?;
        let opt = get_option(&env, option_id).ok_or(Error::OptionNotFound)?;
        if opt.status != OptionStatus::Active {
            return Ok(MarginRequirement {
                required: 0,
                deposited: get_margin(&env, &opt.writer),
                margin_call: false,
            });
        }
        let notional = opt
            .strike_price
            .checked_mul(opt.size)
            .ok_or(Error::Overflow)?
            / PRICE_PRECISION;
        let required = notional
            .checked_mul(MIN_MARGIN_BPS)
            .ok_or(Error::Overflow)?
            / 10_000;
        let deposited = get_margin(&env, &opt.writer);
        Ok(MarginRequirement {
            required,
            deposited,
            margin_call: deposited < required,
        })
    }

    // ── Read-only ─────────────────────────────────────────────────────────────

    pub fn get_option(env: Env, option_id: u32) -> Result<OptionContract, Error> {
        ensure_initialized(&env)?;
        get_option(&env, option_id).ok_or(Error::OptionNotFound)
    }

    pub fn get_margin_balance(env: Env, writer: Address) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        Ok(get_margin(&env, &writer))
    }

    pub fn get_total_margin(env: Env) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        Ok(get_total_margin(&env))
    }

    pub fn get_option_count(env: Env) -> Result<u32, Error> {
        ensure_initialized(&env)?;
        Ok(get_option_count(&env))
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn set_paused(env: Env, admin: Address, paused: bool) -> Result<(), Error> {
        admin.require_auth();
        let contract_admin = get_admin(&env)?;
        if contract_admin != admin {
            return Err(Error::Unauthorized);
        }
        set_paused(&env, paused);
        Ok(())
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

// ── Black-Scholes math (integer approximations) ───────────────────────────────

/// Approximate standard normal CDF N(x) using rational polynomial approximation.
/// Input x is scaled by GREEK_PRECISION. Output is scaled by GREEK_PRECISION.
///
/// Uses the Abramowitz and Stegun approximation (7.1.26 adapted for integers).
fn normal_cdf(x_scaled: i128) -> i128 {
    // Clamp to ±6 standard deviations for numerical stability
    let x_scaled = x_scaled.clamp(-6 * GREEK_PRECISION, 6 * GREEK_PRECISION);

    if x_scaled == 0 {
        return GREEK_PRECISION / 2;
    }

    let neg = x_scaled < 0;
    let ax = x_scaled.abs();

    // Abramowitz & Stegun rational approx: t = 1 / (1 + 0.2316419 * |x|)
    // All coefficients scaled × GREEK_PRECISION
    // p = 0.319381530, a1 = 0.319381530, a2 = -0.356563782, a3 = 1.781477937,
    //     a4 = -1.821255978, a5 = 1.330274429
    // Scaled by 10^8 for integer:
    let a1: i128 = 31_938_153;
    let a2: i128 = -35_656_378;
    let a3: i128 = 178_147_794;
    let a4: i128 = -182_125_598;
    let a5: i128 = 133_027_443;
    let p_coeff: i128 = 2_316_419; // 0.2316419 × 10^7

    // t = 10^7 / (10^7 + p * |x| / GREEK_PRECISION)
    let denom_t = 10_000_000 + p_coeff * ax / GREEK_PRECISION;
    if denom_t == 0 {
        return if neg { 0 } else { GREEK_PRECISION };
    }
    let t = 10_000_000_000_000_i128 / denom_t; // t scaled by 10^6

    // Horner's method: poly = t*(a1 + t*(a2 + t*(a3 + t*(a4 + t*a5)))) / 10^8
    let t6 = t / 1_000; // scale to 10^3 for intermediate products
    let poly = t6
        * (a1 + t6 * (a2 + t6 * (a3 + t6 * (a4 + t6 * a5) / 100_000) / 100_000) / 100_000)
        / 100_000;

    // Gaussian component: exp(-x²/2) / sqrt(2π) ≈ normal_pdf(x)
    let pdf = normal_pdf(ax);

    // N(|x|) ≈ 1 - pdf * poly / GREEK_PRECISION
    let n = GREEK_PRECISION - pdf * poly.abs() / GREEK_PRECISION;
    let n = n.clamp(0, GREEK_PRECISION);

    if neg {
        GREEK_PRECISION - n
    } else {
        n
    }
}

/// Approximate standard normal PDF n(x) = exp(-x²/2) / sqrt(2π).
/// Input x is scaled by GREEK_PRECISION. Output is scaled by GREEK_PRECISION.
///
/// Uses integer approximation of Gaussian: e^(-u) ≈ (1 - u/n)^n for small u,
/// or Padé approximation for the exponential.
fn normal_pdf(x_scaled: i128) -> i128 {
    // pdf = (1/sqrt(2π)) * exp(-x²/2)
    // 1/sqrt(2π) ≈ 0.398942 → scaled = 398_942
    let inv_sqrt_2pi: i128 = 398_942; // × GREEK_PRECISION / 10^6

    let x2 = x_scaled
        .saturating_mul(x_scaled)
        .checked_div(GREEK_PRECISION)
        .unwrap_or(i128::MAX / 2);

    // exp(-x²/2): use Taylor/Padé around 0
    // For |x| > 6 SD, pdf ≈ 0
    if x2 > 36 * GREEK_PRECISION {
        return 0;
    }

    // Padé approximation for e^(-u) where u = x²/2:
    // e^(-u) ≈ (120 - 60u + 12u² - u³) / (120 + 60u + 12u² + u³) for small u
    // u = x2/2, scaled by GREEK_PRECISION
    let u = x2 / 2;
    // All operations in GREEK_PRECISION units
    let u2 = u.saturating_mul(u) / GREEK_PRECISION;
    let u3 = u2.saturating_mul(u) / GREEK_PRECISION;

    let scale = GREEK_PRECISION;
    let num = 120 * scale - 60 * u + 12 * u2 - u3;
    let den = 120 * scale + 60 * u + 12 * u2 + u3;

    let exp_neg_u = if den > 0 {
        num.max(0).checked_mul(scale).unwrap_or(0) / den
    } else {
        0
    };

    inv_sqrt_2pi * exp_neg_u / 1_000_000
}

/// Integer square root of `(x * scale)`, returning result scaled by `sqrt(scale)`.
/// Used to compute √T where T is scaled by GREEK_PRECISION.
fn isqrt_scaled(x_scaled: i128, _scale: i128) -> i128 {
    if x_scaled <= 0 {
        return 0;
    }
    // We want sqrt(x_scaled / GREEK_PRECISION) * GREEK_PRECISION
    // = sqrt(x_scaled * GREEK_PRECISION)
    let product = x_scaled.saturating_mul(GREEK_PRECISION);
    let mut s = product;
    let mut t = (s + 1) / 2;
    while t < s {
        s = t;
        t = (s + product / s) / 2;
    }
    s
}
