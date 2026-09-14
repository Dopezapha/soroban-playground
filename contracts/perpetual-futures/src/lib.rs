// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Perpetual Futures Contract
//!
//! Soroban smart contract for trading perpetual futures with customizable leverage,
//! funding rate mechanisms, margin validation, position management, and liquidations.

#![no_std]

mod storage;
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

use crate::storage::{
    get_admin, get_funding_rate, get_position, get_position_count, is_initialized, is_paused,
    set_admin, set_funding_rate, set_initialized, set_paused, set_position, set_position_count,
};
use crate::types::{Error, FundingRate, Position, PositionStatus};

const MAX_LEVERAGE: u32 = 100;
const MAINTENANCE_MARGIN_BPS: i128 = 500; // 5%

#[contract]
pub struct PerpetualFutures;

#[contractimpl]
impl PerpetualFutures {
    pub fn initialize(env: Env, admin: Address, initial_price: i128) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        if initial_price <= 0 {
            return Err(Error::InvalidPrice);
        }
        admin.require_auth();

        set_admin(&env, &admin);
        set_initialized(&env);
        set_paused(&env, false);
        set_position_count(&env, 0);

        let funding = FundingRate {
            mark_price: initial_price,
            index_price: initial_price,
            rate_bps: 0,
            last_update: env.ledger().timestamp(),
        };
        set_funding_rate(&env, &funding);

        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    pub fn set_paused(env: Env, admin: Address, paused: bool) -> Result<(), Error> {
        Self::assert_admin(&env, &admin)?;
        set_paused(&env, paused);
        env.events().publish((symbol_short!("pause"),), paused);
        Ok(())
    }

    pub fn open_position(
        env: Env,
        trader: Address,
        is_long: bool,
        size: i128,
        leverage: u32,
        collateral: i128,
    ) -> Result<u64, Error> {
        Self::assert_not_paused(&env)?;
        Self::assert_initialized(&env)?;
        trader.require_auth();

        if size <= 0 {
            return Err(Error::InvalidSize);
        }
        if leverage == 0 || leverage > MAX_LEVERAGE {
            return Err(Error::InvalidLeverage);
        }
        if collateral <= 0 {
            return Err(Error::InsufficientMargin);
        }

        let funding = get_funding_rate(&env)?;
        let id = get_position_count(&env) + 1;

        let position = Position {
            id,
            trader: trader.clone(),
            is_long,
            size,
            leverage,
            entry_price: funding.mark_price,
            collateral,
            status: PositionStatus::Active,
        };

        set_position(&env, &position);
        set_position_count(&env, id);

        env.events()
            .publish((symbol_short!("open_pos"), id), (trader, is_long, size));
        Ok(id)
    }

    pub fn close_position(
        env: Env,
        trader: Address,
        position_id: u64,
        exit_price: i128,
    ) -> Result<i128, Error> {
        Self::assert_not_paused(&env)?;
        Self::assert_initialized(&env)?;
        trader.require_auth();

        if exit_price <= 0 {
            return Err(Error::InvalidPrice);
        }

        let mut pos = get_position(&env, position_id)?;
        if pos.trader != trader {
            return Err(Error::Unauthorized);
        }
        if pos.status != PositionStatus::Active {
            return Err(Error::PositionNotActive);
        }

        let pnl = if pos.is_long {
            (exit_price - pos.entry_price) * pos.size / pos.entry_price
        } else {
            (pos.entry_price - exit_price) * pos.size / pos.entry_price
        };

        pos.status = PositionStatus::Closed;
        set_position(&env, &pos);

        let net_settlement = pos.collateral + pnl;
        env.events().publish(
            (symbol_short!("close_pos"), position_id),
            (trader, net_settlement),
        );

        Ok(net_settlement)
    }

    pub fn update_funding_rate(
        env: Env,
        admin: Address,
        mark_price: i128,
        index_price: i128,
    ) -> Result<i32, Error> {
        Self::assert_admin(&env, &admin)?;
        if mark_price <= 0 || index_price <= 0 {
            return Err(Error::InvalidPrice);
        }

        let rate_bps = ((mark_price - index_price) * 10_000 / index_price) as i32;
        let funding = FundingRate {
            mark_price,
            index_price,
            rate_bps,
            last_update: env.ledger().timestamp(),
        };

        set_funding_rate(&env, &funding);
        env.events()
            .publish((symbol_short!("fund_rate"),), rate_bps);

        Ok(rate_bps)
    }

    pub fn liquidate_position(
        env: Env,
        liquidator: Address,
        position_id: u64,
        current_price: i128,
    ) -> Result<(), Error> {
        Self::assert_not_paused(&env)?;
        Self::assert_initialized(&env)?;
        liquidator.require_auth();

        if current_price <= 0 {
            return Err(Error::InvalidPrice);
        }

        let mut pos = get_position(&env, position_id)?;
        if pos.status != PositionStatus::Active {
            return Err(Error::PositionNotActive);
        }

        let pnl = if pos.is_long {
            (current_price - pos.entry_price) * pos.size / pos.entry_price
        } else {
            (pos.entry_price - current_price) * pos.size / pos.entry_price
        };

        let remaining_margin = pos.collateral + pnl;
        let maintenance_threshold = pos.collateral * MAINTENANCE_MARGIN_BPS / 10_000;

        if remaining_margin > maintenance_threshold {
            return Err(Error::InsufficientMargin);
        }

        pos.status = PositionStatus::Liquidated;
        set_position(&env, &pos);

        env.events()
            .publish((symbol_short!("liquidate"), position_id), liquidator);

        Ok(())
    }

    pub fn get_position(env: Env, position_id: u64) -> Result<Position, Error> {
        get_position(&env, position_id)
    }

    pub fn get_funding_rate(env: Env) -> Result<FundingRate, Error> {
        get_funding_rate(&env)
    }

    pub fn position_count(env: Env) -> u64 {
        get_position_count(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        is_paused(&env)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        get_admin(&env)
    }

    fn assert_admin(env: &Env, admin: &Address) -> Result<(), Error> {
        Self::assert_initialized(env)?;
        admin.require_auth();
        let stored_admin = get_admin(env)?;
        if stored_admin != *admin {
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

    fn assert_not_paused(env: &Env) -> Result<(), Error> {
        if is_paused(env) {
            return Err(Error::ContractPaused);
        }
        Ok(())
    }
}
