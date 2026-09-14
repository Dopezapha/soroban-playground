#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String,
};

const INSTANCE_BUMP_THRESHOLD: u32 = 17_280;
const INSTANCE_EXTEND_TO: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AdNetworkError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidParameters = 4,
    CampaignNotFound = 5,
    PublisherNotFound = 6,
    ImpressionNotFound = 7,
    AlreadyVerified = 8,
    InsufficientBudget = 9,
    InvalidImpression = 10,
    DuplicateImpression = 11,
    CampaignNotActive = 12,
    PaymentFailed = 13,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Campaign {
    pub id: u32,
    pub advertiser: Address,
    pub name: String,
    pub budget: i128,
    pub spent: i128,
    pub cpm_rate: i128,
    pub start_time: u64,
    pub end_time: u64,
    pub active: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publisher {
    pub id: u32,
    pub address: Address,
    pub domain: String,
    pub reputation_score: u32,
    pub total_earned: i128,
    pub impressions_served: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Impression {
    pub id: u32,
    pub campaign_id: u32,
    pub publisher_id: u32,
    pub user_hash: String,
    pub timestamp: u64,
    pub verified: bool,
    pub paid: bool,
    pub amount: i128,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    CampaignCount,
    PublisherCount,
    ImpressionCount,
    Campaign(u32),
    Publisher(u32),
    Impression(u32),
    CampaignImpressions(u32, u32),
    PublisherImpressions(u32, u32),
    ImpressionHash(String),
    PublisherByAddress(Address),
}

#[contract]
pub struct AdNetworkContract;

#[contractimpl]
impl AdNetworkContract {
    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), AdNetworkError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(AdNetworkError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::CampaignCount, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::PublisherCount, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::ImpressionCount, &0u32);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_BUMP_THRESHOLD, INSTANCE_EXTEND_TO);
        Ok(())
    }

    pub fn create_campaign(
        env: Env,
        advertiser: Address,
        name: String,
        budget: i128,
        cpm_rate: i128,
        start_time: u64,
        end_time: u64,
    ) -> Result<u32, AdNetworkError> {
        advertiser.require_auth();

        if budget <= 0 || cpm_rate <= 0 || start_time >= end_time {
            return Err(AdNetworkError::InvalidParameters);
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CampaignCount)
            .unwrap_or(0);
        let id = count + 1;

        let campaign = Campaign {
            id,
            advertiser: advertiser.clone(),
            name,
            budget,
            spent: 0,
            cpm_rate,
            start_time,
            end_time,
            active: true,
        };

        env.storage()
            .instance()
            .set(&DataKey::Campaign(id), &campaign);
        env.storage().instance().set(&DataKey::CampaignCount, &id);

        env.events()
            .publish((symbol_short!("ad"), symbol_short!("campaign")), id);
        Ok(id)
    }

    pub fn register_publisher(
        env: Env,
        admin: Address,
        publisher_address: Address,
        domain: String,
    ) -> Result<u32, AdNetworkError> {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AdNetworkError::NotInitialized)?;
        if admin != stored_admin {
            return Err(AdNetworkError::Unauthorized);
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PublisherCount)
            .unwrap_or(0);
        let id = count + 1;

        let publisher = Publisher {
            id,
            address: publisher_address.clone(),
            domain,
            reputation_score: 100,
            total_earned: 0,
            impressions_served: 0,
        };

        env.storage()
            .instance()
            .set(&DataKey::Publisher(id), &publisher);
        env.storage().instance().set(&DataKey::PublisherCount, &id);
        env.storage()
            .instance()
            .set(&DataKey::PublisherByAddress(publisher_address), &id);

        env.events()
            .publish((symbol_short!("ad"), symbol_short!("publisher")), id);
        Ok(id)
    }

    pub fn record_impression(
        env: Env,
        campaign_id: u32,
        publisher_id: u32,
        user_hash: String,
    ) -> Result<u32, AdNetworkError> {
        let campaign: Campaign = env
            .storage()
            .instance()
            .get(&DataKey::Campaign(campaign_id))
            .ok_or(AdNetworkError::CampaignNotFound)?;

        if !campaign.active {
            return Err(AdNetworkError::CampaignNotActive);
        }

        let now = env.ledger().timestamp();
        if now < campaign.start_time || now > campaign.end_time {
            return Err(AdNetworkError::CampaignNotActive);
        }

        if env
            .storage()
            .instance()
            .has(&DataKey::ImpressionHash(user_hash.clone()))
        {
            return Err(AdNetworkError::DuplicateImpression);
        }

        let _publisher: Publisher = env
            .storage()
            .instance()
            .get(&DataKey::Publisher(publisher_id))
            .ok_or(AdNetworkError::PublisherNotFound)?;

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ImpressionCount)
            .unwrap_or(0);
        let id = count + 1;

        let impression = Impression {
            id,
            campaign_id,
            publisher_id,
            user_hash: user_hash.clone(),
            timestamp: now,
            verified: false,
            paid: false,
            amount: 0,
        };

        env.storage()
            .instance()
            .set(&DataKey::Impression(id), &impression);
        env.storage().instance().set(&DataKey::ImpressionCount, &id);
        env.storage()
            .instance()
            .set(&DataKey::ImpressionHash(user_hash), &id);

        env.events()
            .publish((symbol_short!("ad"), symbol_short!("impress")), id);
        Ok(id)
    }

    pub fn verify_impression(
        env: Env,
        admin: Address,
        impression_id: u32,
    ) -> Result<(), AdNetworkError> {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AdNetworkError::NotInitialized)?;
        if admin != stored_admin {
            return Err(AdNetworkError::Unauthorized);
        }

        let key = DataKey::Impression(impression_id);
        let mut impression: Impression = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(AdNetworkError::ImpressionNotFound)?;

        if impression.verified {
            return Err(AdNetworkError::AlreadyVerified);
        }

        let campaign: Campaign = env
            .storage()
            .instance()
            .get(&DataKey::Campaign(impression.campaign_id))
            .ok_or(AdNetworkError::CampaignNotFound)?;

        let amount = campaign.cpm_rate;

        impression.verified = true;
        impression.amount = amount;
        env.storage().instance().set(&key, &impression);

        let mut campaign = campaign;
        campaign.spent += amount;
        env.storage()
            .instance()
            .set(&DataKey::Campaign(campaign.id), &campaign);

        let mut publisher: Publisher = env
            .storage()
            .instance()
            .get(&DataKey::Publisher(impression.publisher_id))
            .ok_or(AdNetworkError::PublisherNotFound)?;
        publisher.total_earned += amount;
        publisher.impressions_served += 1;
        env.storage()
            .instance()
            .set(&DataKey::Publisher(publisher.id), &publisher);

        let _token: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(AdNetworkError::NotInitialized)?;

        env.events().publish(
            (symbol_short!("ad"), symbol_short!("verified")),
            (impression_id, amount),
        );

        Ok(())
    }

    pub fn get_campaign(env: Env, campaign_id: u32) -> Result<Campaign, AdNetworkError> {
        env.storage()
            .instance()
            .get(&DataKey::Campaign(campaign_id))
            .ok_or(AdNetworkError::CampaignNotFound)
    }

    pub fn get_publisher(env: Env, publisher_id: u32) -> Result<Publisher, AdNetworkError> {
        env.storage()
            .instance()
            .get(&DataKey::Publisher(publisher_id))
            .ok_or(AdNetworkError::PublisherNotFound)
    }

    pub fn get_impression(env: Env, impression_id: u32) -> Result<Impression, AdNetworkError> {
        env.storage()
            .instance()
            .get(&DataKey::Impression(impression_id))
            .ok_or(AdNetworkError::ImpressionNotFound)
    }
}

mod test;
