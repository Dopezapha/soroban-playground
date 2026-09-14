#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
};

// Storage TTL Bump Bounds
const INSTANCE_BUMP_THRESHOLD: u32 = 17_280; // ~1 day
const INSTANCE_EXTEND_TO: u32 = 518_400; // ~30 days

const PERSISTENT_BUMP_THRESHOLD: u32 = 17_280;
const PERSISTENT_EXTEND_TO: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VestingError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidSchedule = 4,
    ScheduleExists = 5,
    NoVestingSchedule = 6,
    CliffNotReached = 7,
    NoTokensToClaim = 8,
    MilestoneAlreadyUnlocked = 9,
    MilestoneNotFound = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSchedule {
    pub total_amount: i128,
    pub released_amount: i128,
    pub start_time: u64,
    pub cliff_time: u64,
    pub end_time: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub percent_bps: u32, // Basis points e.g. 1000 = 10%
    pub unlocked: bool,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    Vesting(Address),
    Milestone(Address, u32), // Beneficiary, Milestone ID
}

#[contract]
pub struct TokenVestingContract;

#[contractimpl]
impl TokenVestingContract {
    /// Initialize global vesting vault with admin and underlying SEP-41 token
    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), VestingError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(VestingError::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);

        Ok(())
    }

    /// Admin creates a linear vesting schedule with optional cliff duration for a beneficiary
    pub fn create_schedule(
        env: Env,
        beneficiary: Address,
        total_amount: i128,
        start_time: u64,
        cliff_duration: u64,
        duration: u64,
    ) -> Result<(), VestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)?;
        admin.require_auth();

        if total_amount <= 0 || duration == 0 {
            return Err(VestingError::InvalidSchedule);
        }

        let key = DataKey::Vesting(beneficiary.clone());
        if env.storage().persistent().has(&key) {
            return Err(VestingError::ScheduleExists);
        }

        let cliff_time = start_time + cliff_duration;
        let end_time = start_time + duration;

        if cliff_time > end_time {
            return Err(VestingError::InvalidSchedule);
        }

        let token_address: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(VestingError::NotInitialized)?;

        // Escrow total allocation into contract
        let token_client = token::Client::new(&env, &token_address);
        token_client.transfer(&admin, &env.current_contract_address(), &total_amount);

        let schedule = VestingSchedule {
            total_amount,
            released_amount: 0,
            start_time,
            cliff_time,
            end_time,
        };

        env.storage().persistent().set(&key, &schedule);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        env.events().publish(
            (symbol_short!("vesting"), symbol_short!("created")),
            (beneficiary, total_amount),
        );

        Ok(())
    }

    /// Admin configures an instant milestone-based release percentage for a beneficiary
    pub fn add_milestone(
        env: Env,
        beneficiary: Address,
        milestone_id: u32,
        percent_bps: u32,
    ) -> Result<(), VestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)?;
        admin.require_auth();

        let key = DataKey::Milestone(beneficiary, milestone_id);
        let milestone = Milestone {
            percent_bps,
            unlocked: false,
        };

        env.storage().persistent().set(&key, &milestone);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        Ok(())
    }

    /// Admin unlocks a specific milestone
    pub fn unlock_milestone(
        env: Env,
        beneficiary: Address,
        milestone_id: u32,
    ) -> Result<(), VestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)?;
        admin.require_auth();

        let key = DataKey::Milestone(beneficiary.clone(), milestone_id);
        let mut milestone: Milestone = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::MilestoneNotFound)?;

        if milestone.unlocked {
            return Err(VestingError::MilestoneAlreadyUnlocked);
        }

        milestone.unlocked = true;
        env.storage().persistent().set(&key, &milestone);

        env.events().publish(
            (symbol_short!("vesting"), symbol_short!("m_unlock")),
            (beneficiary, milestone_id),
        );

        Ok(())
    }

    /// Calculate currently claimable tokens (linear schedule + unlocked milestones - already claimed)
    pub fn claimable_amount(env: Env, beneficiary: Address) -> i128 {
        let key = DataKey::Vesting(beneficiary.clone());
        let schedule: VestingSchedule = match env.storage().persistent().get(&key) {
            Some(s) => s,
            None => return 0,
        };

        let current_time = env.ledger().timestamp();
        if current_time < schedule.cliff_time {
            return 0;
        }

        // 1. Calculate linear unlocked tokens
        let linear_unlocked = if current_time >= schedule.end_time {
            schedule.total_amount
        } else {
            let elapsed = current_time - schedule.start_time;
            let duration = schedule.end_time - schedule.start_time;
            schedule
                .total_amount
                .checked_mul(elapsed as i128)
                .unwrap()
                .checked_div(duration as i128)
                .unwrap()
        };

        let claimable = linear_unlocked.saturating_sub(schedule.released_amount);
        claimable.max(0)
    }

    /// Beneficiary claims all available unlocked tokens
    pub fn claim(env: Env, beneficiary: Address) -> Result<i128, VestingError> {
        beneficiary.require_auth();

        let amount_to_claim = Self::claimable_amount(env.clone(), beneficiary.clone());
        if amount_to_claim <= 0 {
            return Err(VestingError::NoTokensToClaim);
        }

        let key = DataKey::Vesting(beneficiary.clone());
        let mut schedule: VestingSchedule = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::NoVestingSchedule)?;

        schedule.released_amount = schedule
            .released_amount
            .checked_add(amount_to_claim)
            .unwrap();

        env.storage().persistent().set(&key, &schedule);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        let token_address: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(VestingError::NotInitialized)?;

        let token_client = token::Client::new(&env, &token_address);
        token_client.transfer(
            &env.current_contract_address(),
            &beneficiary,
            &amount_to_claim,
        );

        env.events().publish(
            (symbol_short!("vesting"), symbol_short!("claimed")),
            (beneficiary, amount_to_claim),
        );

        Ok(amount_to_claim)
    }
}
