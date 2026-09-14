#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String,
};

const INSTANCE_BUMP_THRESHOLD: u32 = 17_280;
const INSTANCE_EXTEND_TO: u32 = 518_400;
const PERSISTENT_BUMP_THRESHOLD: u32 = 17_280;
const PERSISTENT_EXTEND_TO: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum IndexTokenError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidParameters = 4,
    PoolNotFound = 5,
    AssetNotFound = 6,
    InvalidWeight = 7,
    RebalanceAlreadyActive = 8,
    NoRebalancePending = 9,
    SlippageExceeded = 10,
    InsufficientBalance = 11,
    AssetAlreadyExists = 12,
    InvalidTargetWeight = 13,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub address: Address,
    pub symbol: String,
    pub current_weight_bps: u32,
    pub target_weight_bps: u32,
    pub balance: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexPool {
    pub id: u32,
    pub name: String,
    pub admin: Address,
    pub total_supply: i128,
    pub asset_count: u32,
    pub rebalance_threshold_bps: u32,
    pub last_rebalance: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebalanceProposal {
    pub pool_id: u32,
    pub proposed_at: u64,
    pub executed: bool,
}

#[contracttype]
pub enum DataKey {
    Admin,
    PoolCount,
    Pool(u32),
    Asset(u32, u32),
    AssetCount(u32),
    RebalanceProposal(u32),
    TotalValueLocked(u32),
    ShareBalance(u32, Address),
}

#[contract]
pub struct IndexTokenContract;

#[contractimpl]
impl IndexTokenContract {
    pub fn initialize(env: Env, admin: Address) -> Result<(), IndexTokenError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(IndexTokenError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::PoolCount, &0u32);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);
        Ok(())
    }

    pub fn create_pool(
        env: Env,
        name: String,
        rebalance_threshold_bps: u32,
    ) -> Result<u32, IndexTokenError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(IndexTokenError::NotInitialized)?;
        admin.require_auth();

        if rebalance_threshold_bps == 0 || rebalance_threshold_bps > 5000 {
            return Err(IndexTokenError::InvalidParameters);
        }

        let pool_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PoolCount)
            .unwrap_or(0);
        let pool_id = pool_count + 1;

        let pool = IndexPool {
            id: pool_id,
            name,
            admin,
            total_supply: 0,
            asset_count: 0,
            rebalance_threshold_bps,
            last_rebalance: env.ledger().timestamp(),
            active: true,
        };

        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);
        env.storage().instance().set(&DataKey::PoolCount, &pool_id);
        env.storage()
            .instance()
            .set(&DataKey::AssetCount(pool_id), &0u32);
        env.storage()
            .instance()
            .set(&DataKey::TotalValueLocked(pool_id), &0i128);

        env.events()
            .publish((symbol_short!("idx"), symbol_short!("pool")), pool_id);
        Ok(pool_id)
    }

    pub fn add_asset(
        env: Env,
        pool_id: u32,
        asset_address: Address,
        symbol: String,
        target_weight_bps: u32,
    ) -> Result<u32, IndexTokenError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(IndexTokenError::NotInitialized)?;
        admin.require_auth();

        if target_weight_bps == 0 || target_weight_bps > 10000 {
            return Err(IndexTokenError::InvalidWeight);
        }

        let mut pool: IndexPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(IndexTokenError::PoolNotFound)?;

        if !pool.active {
            return Err(IndexTokenError::InvalidParameters);
        }

        let asset_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::AssetCount(pool_id))
            .unwrap_or(0);

        let asset = Asset {
            address: asset_address,
            symbol,
            current_weight_bps: 0,
            target_weight_bps,
            balance: 0,
        };

        let asset_id = asset_count + 1;
        env.storage()
            .persistent()
            .set(&DataKey::Asset(pool_id, asset_id), &asset);
        env.storage().persistent().extend_ttl(
            &DataKey::Asset(pool_id, asset_id),
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );
        env.storage()
            .instance()
            .set(&DataKey::AssetCount(pool_id), &asset_id);

        pool.asset_count = asset_id;
        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);

        env.events()
            .publish((symbol_short!("idx"), symbol_short!("asset")), pool_id);
        Ok(asset_id)
    }

    pub fn deposit(
        env: Env,
        pool_id: u32,
        depositor: Address,
        amount: i128,
    ) -> Result<i128, IndexTokenError> {
        depositor.require_auth();

        if amount <= 0 {
            return Err(IndexTokenError::InvalidParameters);
        }

        let mut pool: IndexPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(IndexTokenError::PoolNotFound)?;

        let tvl: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalValueLocked(pool_id))
            .unwrap_or(0);

        let shares = if pool.total_supply == 0 {
            amount
        } else {
            (amount * pool.total_supply) / tvl
        };

        pool.total_supply += shares;
        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);

        let current: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::ShareBalance(pool_id, depositor.clone()))
            .unwrap_or(0);
        env.storage().persistent().set(
            &DataKey::ShareBalance(pool_id, depositor.clone()),
            &(current + shares),
        );

        let new_tvl = tvl + amount;
        env.storage()
            .instance()
            .set(&DataKey::TotalValueLocked(pool_id), &new_tvl);

        env.events().publish(
            (symbol_short!("idx"), symbol_short!("deposit")),
            (pool_id, depositor, amount, shares),
        );

        Ok(shares)
    }

    pub fn withdraw(
        env: Env,
        pool_id: u32,
        investor: Address,
        shares: i128,
    ) -> Result<i128, IndexTokenError> {
        investor.require_auth();

        if shares <= 0 {
            return Err(IndexTokenError::InvalidParameters);
        }

        let current: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::ShareBalance(pool_id, investor.clone()))
            .unwrap_or(0);

        if current < shares {
            return Err(IndexTokenError::InsufficientBalance);
        }

        let pool: IndexPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(IndexTokenError::PoolNotFound)?;

        let tvl: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalValueLocked(pool_id))
            .unwrap_or(0);

        let amount = (shares * tvl) / pool.total_supply;

        env.storage().persistent().set(
            &DataKey::ShareBalance(pool_id, investor.clone()),
            &(current - shares),
        );

        let mut pool = pool;
        pool.total_supply -= shares;
        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);

        let new_tvl = tvl - amount;
        env.storage()
            .instance()
            .set(&DataKey::TotalValueLocked(pool_id), &new_tvl);

        env.events().publish(
            (symbol_short!("idx"), symbol_short!("withdraw")),
            (pool_id, investor, amount, shares),
        );

        Ok(amount)
    }

    pub fn propose_rebalance(env: Env, pool_id: u32) -> Result<(), IndexTokenError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(IndexTokenError::NotInitialized)?;
        admin.require_auth();

        if env
            .storage()
            .instance()
            .has(&DataKey::RebalanceProposal(pool_id))
        {
            return Err(IndexTokenError::RebalanceAlreadyActive);
        }

        let proposal = RebalanceProposal {
            pool_id,
            proposed_at: env.ledger().timestamp(),
            executed: false,
        };
        env.storage()
            .instance()
            .set(&DataKey::RebalanceProposal(pool_id), &proposal);

        env.events()
            .publish((symbol_short!("idx"), symbol_short!("rebal")), pool_id);
        Ok(())
    }

    pub fn execute_rebalance(
        env: Env,
        pool_id: u32,
        _max_slippage_bps: u32,
    ) -> Result<(), IndexTokenError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(IndexTokenError::NotInitialized)?;
        admin.require_auth();

        let proposal: RebalanceProposal = env
            .storage()
            .instance()
            .get(&DataKey::RebalanceProposal(pool_id))
            .ok_or(IndexTokenError::NoRebalancePending)?;

        if proposal.executed {
            return Err(IndexTokenError::RebalanceAlreadyActive);
        }

        let mut pool: IndexPool = env
            .storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(IndexTokenError::PoolNotFound)?;

        let asset_count = pool.asset_count;
        let mut total_target: u32 = 0;

        let mut i = 1;
        while i <= asset_count {
            let mut asset: Asset = env
                .storage()
                .persistent()
                .get(&DataKey::Asset(pool_id, i))
                .unwrap();
            total_target += asset.target_weight_bps;
            asset.current_weight_bps = asset.target_weight_bps;
            env.storage()
                .persistent()
                .set(&DataKey::Asset(pool_id, i), &asset);
            i += 1;
        }

        if total_target != 10000 {
            return Err(IndexTokenError::InvalidTargetWeight);
        }

        pool.last_rebalance = env.ledger().timestamp();
        env.storage().instance().set(&DataKey::Pool(pool_id), &pool);

        let mut p = proposal;
        p.executed = true;
        env.storage()
            .instance()
            .set(&DataKey::RebalanceProposal(pool_id), &p);

        env.events()
            .publish((symbol_short!("idx"), symbol_short!("rebal")), pool_id);
        Ok(())
    }

    pub fn get_pool(env: Env, pool_id: u32) -> Result<IndexPool, IndexTokenError> {
        env.storage()
            .instance()
            .get(&DataKey::Pool(pool_id))
            .ok_or(IndexTokenError::PoolNotFound)
    }

    pub fn get_asset(env: Env, pool_id: u32, asset_id: u32) -> Result<Asset, IndexTokenError> {
        env.storage()
            .persistent()
            .get(&DataKey::Asset(pool_id, asset_id))
            .ok_or(IndexTokenError::AssetNotFound)
    }

    pub fn get_shares(env: Env, pool_id: u32, investor: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::ShareBalance(pool_id, investor))
            .unwrap_or(0)
    }

    pub fn get_tvl(env: Env, pool_id: u32) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalValueLocked(pool_id))
            .unwrap_or(0)
    }
}

mod test;
