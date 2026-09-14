#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, String,
};

const INSTANCE_BUMP_THRESHOLD: u32 = 17_280;
const INSTANCE_EXTEND_TO: u32 = 518_400;
const PERSISTENT_BUMP_THRESHOLD: u32 = 17_280;
const PERSISTENT_EXTEND_TO: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VCVestingError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidParameters = 4,
    PoolNotFound = 7,
    InvalidTranche = 8,
    TrancheNotFound = 9,
    MilestoneAlreadyVoted = 11,
    VotingPeriodEnded = 12,
    InsufficientVotes = 13,
    TrancheAlreadyReleased = 16,
    VotingNotOpen = 17,
    TrancheNotReleased = 20,
    InvalidVotingPeriod = 21,
    InvestorNotFound = 22,
    AlreadyVoted = 23,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tranche {
    pub id: u32,
    pub total_amount: i128,
    pub released_amount: i128,
    pub milestone_description: String,
    pub released: bool,
    pub voting_end_time: u64,
    pub required_votes_bps: u32,
    pub approve_votes: i128,
    pub reject_votes: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Vote {
    pub voter: Address,
    pub pool_id: u32,
    pub tranche_id: u32,
    pub approve: bool,
    pub weight: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VCPool {
    pub id: u32,
    pub name: String,
    pub token: Address,
    pub admin: Address,
    pub total_allocation: i128,
    pub tranche_count: u32,
    pub created_at: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Investor {
    pub address: Address,
    pub allocation: i128,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    PoolCount,
    Pool(u32),
    Tranche(u32, u32),
    Investor(u32, Address),
    InvestorCount(u32),
    VoteRecord(u32, u32, Address),
    TotalInvestorAllocation(u32),
}

#[contract]
pub struct VCVestingContract;

#[contractimpl]
impl VCVestingContract {
    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), VCVestingError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(VCVestingError::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::PoolCount, &0u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);

        Ok(())
    }

    pub fn create_pool(
        env: Env,
        name: String,
        total_allocation: i128,
        tranche_count: u32,
    ) -> Result<u32, VCVestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VCVestingError::NotInitialized)?;
        admin.require_auth();

        if total_allocation <= 0 || tranche_count == 0 || tranche_count > 20 {
            return Err(VCVestingError::InvalidParameters);
        }

        let pool_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PoolCount)
            .unwrap_or(0);
        let pool_id = pool_count + 1;

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(VCVestingError::NotInitialized)?;

        let pool = VCPool {
            id: pool_id,
            name,
            token,
            admin,
            total_allocation,
            tranche_count,
            created_at: env.ledger().timestamp(),
            active: true,
        };

        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);
        env.storage().instance().set(&DataKey::PoolCount, &pool_id);
        env.storage()
            .instance()
            .set(&DataKey::TotalInvestorAllocation(pool_id), &0i128);
        env.storage()
            .instance()
            .set(&DataKey::InvestorCount(pool_id), &0u32);

        env.events()
            .publish((symbol_short!("vc"), symbol_short!("pool")), pool_id);

        Ok(pool_id)
    }

    pub fn add_tranche(
        env: Env,
        pool_id: u32,
        tranche_id: u32,
        amount: i128,
        milestone_description: String,
        required_votes_bps: u32,
    ) -> Result<(), VCVestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VCVestingError::NotInitialized)?;
        admin.require_auth();

        let pool: VCPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(VCVestingError::PoolNotFound)?;

        if !pool.active {
            return Err(VCVestingError::InvalidParameters);
        }

        if tranche_id == 0 || tranche_id > pool.tranche_count {
            return Err(VCVestingError::InvalidTranche);
        }

        if amount <= 0 || required_votes_bps == 0 || required_votes_bps > 10000 {
            return Err(VCVestingError::InvalidParameters);
        }

        let tranche = Tranche {
            id: tranche_id,
            total_amount: amount,
            released_amount: 0,
            milestone_description,
            released: false,
            voting_end_time: 0,
            required_votes_bps,
            approve_votes: 0,
            reject_votes: 0,
        };

        let key = DataKey::Tranche(pool_id, tranche_id);
        env.storage().persistent().set(&key, &tranche);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        env.events()
            .publish((symbol_short!("vc"), symbol_short!("tranche")), pool_id);

        Ok(())
    }

    pub fn add_investor(
        env: Env,
        pool_id: u32,
        investor: Address,
        allocation: i128,
    ) -> Result<(), VCVestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VCVestingError::NotInitialized)?;
        admin.require_auth();

        if allocation <= 0 {
            return Err(VCVestingError::InvalidParameters);
        }

        let _pool: VCPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(VCVestingError::PoolNotFound)?;

        let inv = Investor {
            address: investor.clone(),
            allocation,
        };

        let key = DataKey::Investor(pool_id, investor.clone());
        env.storage().persistent().set(&key, &inv);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::InvestorCount(pool_id))
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::InvestorCount(pool_id), &(count + 1));

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalInvestorAllocation(pool_id))
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TotalInvestorAllocation(pool_id),
            &(total + allocation),
        );

        env.events()
            .publish((symbol_short!("vc"), symbol_short!("investor")), pool_id);

        Ok(())
    }

    pub fn open_voting(
        env: Env,
        pool_id: u32,
        tranche_id: u32,
        voting_duration: u64,
    ) -> Result<(), VCVestingError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(VCVestingError::NotInitialized)?;
        admin.require_auth();

        if voting_duration == 0 || voting_duration > 86400 * 30 {
            return Err(VCVestingError::InvalidVotingPeriod);
        }

        let key = DataKey::Tranche(pool_id, tranche_id);
        let mut tranche: Tranche = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VCVestingError::TrancheNotFound)?;

        if tranche.released {
            return Err(VCVestingError::TrancheAlreadyReleased);
        }

        tranche.voting_end_time = env.ledger().timestamp() + voting_duration;
        env.storage().persistent().set(&key, &tranche);

        env.events().publish(
            (symbol_short!("vc"), symbol_short!("vote_open")),
            tranche_id,
        );

        Ok(())
    }

    pub fn vote(
        env: Env,
        pool_id: u32,
        tranche_id: u32,
        voter: Address,
        approve: bool,
    ) -> Result<(), VCVestingError> {
        voter.require_auth();

        let record_key = DataKey::VoteRecord(pool_id, tranche_id, voter.clone());
        if env.storage().persistent().has(&record_key) {
            return Err(VCVestingError::AlreadyVoted);
        }

        let inv: Investor = env
            .storage()
            .persistent()
            .get(&DataKey::Investor(pool_id, voter.clone()))
            .ok_or(VCVestingError::InvestorNotFound)?;

        let tranche_key = DataKey::Tranche(pool_id, tranche_id);
        let mut tranche: Tranche = env
            .storage()
            .persistent()
            .get(&tranche_key)
            .ok_or(VCVestingError::TrancheNotFound)?;

        if tranche.released {
            return Err(VCVestingError::TrancheAlreadyReleased);
        }

        if env.ledger().timestamp() > tranche.voting_end_time {
            return Err(VCVestingError::VotingPeriodEnded);
        }

        if approve {
            tranche.approve_votes += inv.allocation;
        } else {
            tranche.reject_votes += inv.allocation;
        }
        env.storage().persistent().set(&tranche_key, &tranche);

        let vote = Vote {
            voter,
            pool_id,
            tranche_id,
            approve,
            weight: inv.allocation,
            timestamp: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&record_key, &vote);
        env.storage().persistent().extend_ttl(
            &record_key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        env.events()
            .publish((symbol_short!("vc"), symbol_short!("vote")), tranche_id);

        Ok(())
    }

    pub fn finalize_voting(
        env: Env,
        pool_id: u32,
        tranche_id: u32,
    ) -> Result<bool, VCVestingError> {
        let key = DataKey::Tranche(pool_id, tranche_id);
        let mut tranche: Tranche = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VCVestingError::TrancheNotFound)?;

        if tranche.released {
            return Err(VCVestingError::TrancheAlreadyReleased);
        }

        if env.ledger().timestamp() <= tranche.voting_end_time {
            return Err(VCVestingError::VotingNotOpen);
        }

        let total_investor_allocation: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalInvestorAllocation(pool_id))
            .unwrap_or(0);

        if total_investor_allocation == 0 {
            return Err(VCVestingError::InsufficientVotes);
        }

        let approval_rate = (tranche.approve_votes * 10000) / total_investor_allocation;
        let approved = approval_rate >= (tranche.required_votes_bps as i128);

        if approved {
            tranche.released = true;
            tranche.released_amount = tranche.total_amount;
            env.storage().persistent().set(&key, &tranche);

            let token: Address = env
                .storage()
                .instance()
                .get(&DataKey::Token)
                .ok_or(VCVestingError::NotInitialized)?;
            let token_client = token::Client::new(&env, &token);

            let pool: VCPool = env
                .storage()
                .instance()
                .get(&DataKey::Pool(pool_id))
                .ok_or(VCVestingError::PoolNotFound)?;

            token_client.transfer(
                &env.current_contract_address(),
                &pool.admin,
                &tranche.total_amount,
            );

            env.events().publish(
                (symbol_short!("vc"), symbol_short!("released")),
                (pool_id, tranche_id, tranche.total_amount),
            );
        } else {
            env.events()
                .publish((symbol_short!("vc"), symbol_short!("rejected")), tranche_id);
        }

        Ok(approved)
    }

    pub fn claim_tokens(
        env: Env,
        pool_id: u32,
        tranche_id: u32,
        investor: Address,
    ) -> Result<i128, VCVestingError> {
        investor.require_auth();

        let tranche_key = DataKey::Tranche(pool_id, tranche_id);
        let tranche: Tranche = env
            .storage()
            .persistent()
            .get(&tranche_key)
            .ok_or(VCVestingError::TrancheNotFound)?;

        if !tranche.released {
            return Err(VCVestingError::TrancheNotReleased);
        }

        let inv: Investor = env
            .storage()
            .persistent()
            .get(&DataKey::Investor(pool_id, investor.clone()))
            .ok_or(VCVestingError::InvestorNotFound)?;

        let _pool: VCPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(VCVestingError::PoolNotFound)?;

        let total_allocation: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalInvestorAllocation(pool_id))
            .unwrap_or(0);

        if total_allocation == 0 {
            return Err(VCVestingError::InsufficientVotes);
        }

        let claim_amount = (tranche.total_amount * inv.allocation) / total_allocation;

        if claim_amount <= 0 {
            return Err(VCVestingError::InsufficientVotes);
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(VCVestingError::NotInitialized)?;
        let token_client = token::Client::new(&env, &token);

        token_client.transfer(&env.current_contract_address(), &investor, &claim_amount);

        env.events().publish(
            (symbol_short!("vc"), symbol_short!("claim")),
            (pool_id, tranche_id, claim_amount),
        );

        Ok(claim_amount)
    }

    pub fn get_pool(env: Env, pool_id: u32) -> Result<VCPool, VCVestingError> {
        env.storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(VCVestingError::PoolNotFound)
    }

    pub fn get_tranche(env: Env, pool_id: u32, tranche_id: u32) -> Result<Tranche, VCVestingError> {
        env.storage()
            .persistent()
            .get(&DataKey::Tranche(pool_id, tranche_id))
            .ok_or(VCVestingError::TrancheNotFound)
    }

    pub fn get_investor(
        env: Env,
        pool_id: u32,
        investor: Address,
    ) -> Result<Investor, VCVestingError> {
        env.storage()
            .persistent()
            .get(&DataKey::Investor(pool_id, investor))
            .ok_or(VCVestingError::InvestorNotFound)
    }

    pub fn has_voted(env: Env, pool_id: u32, tranche_id: u32, voter: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::VoteRecord(pool_id, tranche_id, voter))
    }
}

mod test;
