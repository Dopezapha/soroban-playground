#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

// Storage TTL Constants (in ledgers; ~5 seconds per ledger)
const INSTANCE_BUMP_THRESHOLD: u32 = 17_280; // ~1 day
const INSTANCE_EXTEND_TO: u32 = 518_400; // ~30 days

const PERSISTENT_BUMP_THRESHOLD: u32 = 17_280;
const PERSISTENT_EXTEND_TO: u32 = 518_400;

const TEMPORARY_BUMP_THRESHOLD: u32 = 1_200; // ~1 hour
const TEMPORARY_EXTEND_TO: u32 = 17_280; // ~1 day

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum StorageError {
    NotInitialized = 1,
    Unauthorized = 2,
    NonceExpiredOrInvalid = 3,
}

#[contracttype]
pub enum DataKey {
    // Instance storage: Global contract settings & admin
    Admin,
    ProtocolFeeBps,

    // Persistent storage: Important user state requiring long-term retention
    UserBalance(Address),

    // Temporary storage: Ephemeral data like replay attack nonces/oracles
    TxNonce(Address, u64),
}

#[contract]
pub struct DecoupledStorageContract;

#[contractimpl]
impl DecoupledStorageContract {
    /// Initialize global protocol parameters in Instance storage
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::ProtocolFeeBps, &fee_bps);

        // Extend instance storage TTL upon initialization
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);
    }

    /// Manage user balance stored in Persistent storage
    pub fn set_balance(env: Env, user: Address, amount: i128) -> Result<(), StorageError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(StorageError::NotInitialized)?;
        admin.require_auth();

        // Refresh instance storage TTL on interaction
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);

        let key = DataKey::UserBalance(user.clone());
        env.storage().persistent().set(&key, &amount);

        // Extend persistent storage entry TTL
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_BUMP_THRESHOLD,
            PERSISTENT_EXTEND_TO,
        );

        Ok(())
    }

    /// Get user balance and automatically extend its TTL if active
    pub fn get_balance(env: Env, user: Address) -> i128 {
        let key = DataKey::UserBalance(user);
        if let Some(balance) = env.storage().persistent().get::<DataKey, i128>(&key) {
            env.storage().persistent().extend_ttl(
                &key,
                PERSISTENT_BUMP_THRESHOLD,
                PERSISTENT_EXTEND_TO,
            );
            balance
        } else {
            0
        }
    }

    /// Record a short-lived execution nonce in Temporary storage (auto-expiring)
    pub fn consume_nonce(env: Env, user: Address, nonce: u64) -> Result<(), StorageError> {
        user.require_auth();

        let key = DataKey::TxNonce(user.clone(), nonce);
        if env.storage().temporary().has(&key) {
            return Err(StorageError::NonceExpiredOrInvalid);
        }

        env.storage().temporary().set(&key, &true);

        // Extend temporary storage entry TTL for a short duration
        env.storage()
            .temporary()
            .extend_ttl(&key, TEMPORARY_BUMP_THRESHOLD, TEMPORARY_EXTEND_TO);

        env.events().publish(
            (symbol_short!("storage"), symbol_short!("nonce")),
            (user, nonce),
        );

        Ok(())
    }
}
