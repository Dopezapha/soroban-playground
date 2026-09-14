#![no_std]

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    String, Vec,
};

const BPS: i64 = 10_000;
const MAX_ACTIVITY_SCORE: i64 = 1_000_000;
const MAX_CREDENTIAL_SCORE: i64 = 500_000;
const MAX_CREDENTIALS: u32 = 16;
const MAX_BATCH: u32 = 20;
const MAX_DECAY_EPOCHS: u64 = 128;
const MAX_CREDENTIAL_LIFETIME: u64 = 157_680_000; // five years

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    InvalidConfig = 4,
    Paused = 5,
    AlreadyRegistered = 6,
    SubjectNotFound = 7,
    ReporterNotFound = 8,
    IssuerNotFound = 9,
    InvalidPoints = 10,
    EventAlreadyProcessed = 11,
    RateLimitExceeded = 12,
    CredentialAlreadyActive = 13,
    CredentialNotFound = 14,
    InvalidCredential = 15,
    CredentialLimitExceeded = 16,
    BatchLimitExceeded = 17,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReporterConfig {
    pub active: bool,
    pub weight_bps: u32,
    pub max_points_per_event: i64,
    pub max_points_per_epoch: i64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuerConfig {
    pub active: bool,
    pub max_credential_weight: i64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Credential {
    pub issuer: Address,
    pub kind: String,
    pub weight: i64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub revoked: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreMatrix {
    pub activity_score: i64,
    pub credential_score: i64,
    pub active_credentials: u32,
    pub confidence_bps: u32,
    pub final_score: i64,
}

#[contracttype]
#[derive(Clone)]
struct SubjectRecord {
    activity_score: i64,
    last_decay_at: u64,
    registered_at: u64,
    positive_events: u64,
    negative_events: u64,
    issuers: Vec<Address>,
}

#[contracttype]
#[derive(Clone)]
enum InstanceKey {
    Admin,
    Initialized,
    Paused,
    DecayBps,
    EpochSeconds,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Subject(Address),
    Reporter(Address),
    Issuer(Address),
    Credential(Address, Address),
    Event(Address, BytesN<32>),
    Bucket(Address, Address, u64),
}

#[contract]
pub struct ReputationContract;

#[contractimpl]
impl ReputationContract {
    /// Initializes decay policy. `decay_bps` must be 1..=10,000 per epoch.
    pub fn initialize(
        env: Env,
        admin: Address,
        epoch_seconds: u64,
        decay_bps: u32,
    ) -> Result<(), Error> {
        if Self::initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        if epoch_seconds == 0 || decay_bps == 0 || decay_bps > BPS as u32 {
            return Err(Error::InvalidConfig);
        }
        admin.require_auth();
        env.storage().instance().set(&InstanceKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&InstanceKey::EpochSeconds, &epoch_seconds);
        env.storage()
            .instance()
            .set(&InstanceKey::DecayBps, &decay_bps);
        env.storage().instance().set(&InstanceKey::Paused, &false);
        env.storage()
            .instance()
            .set(&InstanceKey::Initialized, &true);
        env.events()
            .publish((symbol_short!("init"),), (admin, epoch_seconds, decay_bps));
        Ok(())
    }

    pub fn set_paused(env: Env, paused: bool) -> Result<(), Error> {
        Self::admin(&env)?.require_auth();
        env.storage().instance().set(&InstanceKey::Paused, &paused);
        env.events().publish((symbol_short!("pause"),), paused);
        Ok(())
    }

    pub fn set_reporter(env: Env, reporter: Address, config: ReporterConfig) -> Result<(), Error> {
        Self::admin(&env)?.require_auth();
        if config.weight_bps == 0
            || config.weight_bps > BPS as u32
            || config.max_points_per_event <= 0
            || config.max_points_per_epoch < config.max_points_per_event
        {
            return Err(Error::InvalidConfig);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Reporter(reporter.clone()), &config);
        env.events()
            .publish((symbol_short!("reporter"),), (reporter, config.active));
        Ok(())
    }

    pub fn set_issuer(env: Env, issuer: Address, config: IssuerConfig) -> Result<(), Error> {
        Self::admin(&env)?.require_auth();
        if config.max_credential_weight <= 0 || config.max_credential_weight > MAX_CREDENTIAL_SCORE
        {
            return Err(Error::InvalidConfig);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Issuer(issuer.clone()), &config);
        env.events()
            .publish((symbol_short!("issuer"),), (issuer, config.active));
        Ok(())
    }

    /// Registers a subject. Registration always requires the subject's authorization.
    pub fn register(env: Env, subject: Address) -> Result<(), Error> {
        Self::assert_writable(&env)?;
        subject.require_auth();
        let key = DataKey::Subject(subject.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::AlreadyRegistered);
        }
        let now = env.ledger().timestamp();
        env.storage().persistent().set(
            &key,
            &SubjectRecord {
                activity_score: 0,
                last_decay_at: now,
                registered_at: now,
                positive_events: 0,
                negative_events: 0,
                issuers: Vec::new(&env),
            },
        );
        env.events().publish((symbol_short!("register"),), subject);
        Ok(())
    }

    /// Records a replay-protected, epoch-rate-limited activity observation.
    pub fn record_activity(
        env: Env,
        reporter: Address,
        subject: Address,
        event_id: BytesN<32>,
        points: i64,
    ) -> Result<i64, Error> {
        Self::assert_writable(&env)?;
        reporter.require_auth();
        let config: ReporterConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Reporter(reporter.clone()))
            .ok_or(Error::ReporterNotFound)?;
        if !config.active {
            return Err(Error::ReporterNotFound);
        }
        let magnitude = points.checked_abs().ok_or(Error::InvalidPoints)?;
        if points == 0 || magnitude > config.max_points_per_event {
            return Err(Error::InvalidPoints);
        }
        let event_key = DataKey::Event(reporter.clone(), event_id.clone());
        if env.storage().persistent().has(&event_key) {
            return Err(Error::EventAlreadyProcessed);
        }

        let now = env.ledger().timestamp();
        let epoch_seconds = Self::epoch_seconds(&env)?;
        let epoch = now / epoch_seconds;
        let bucket_key = DataKey::Bucket(reporter.clone(), subject.clone(), epoch);
        let used: i64 = env.storage().temporary().get(&bucket_key).unwrap_or(0);
        let next_used = used
            .checked_add(magnitude)
            .ok_or(Error::RateLimitExceeded)?;
        if next_used > config.max_points_per_epoch {
            return Err(Error::RateLimitExceeded);
        }

        let mut record = Self::subject(&env, &subject)?;
        Self::apply_decay(&env, &mut record, now)?;
        let weighted = points
            .checked_mul(config.weight_bps as i64)
            .ok_or(Error::InvalidPoints)?
            / BPS;
        if weighted == 0 {
            return Err(Error::InvalidPoints);
        }
        record.activity_score = record
            .activity_score
            .saturating_add(weighted)
            .clamp(0, MAX_ACTIVITY_SCORE);
        if points > 0 {
            record.positive_events = record.positive_events.saturating_add(1);
        } else {
            record.negative_events = record.negative_events.saturating_add(1);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Subject(subject.clone()), &record);
        env.storage().persistent().set(&event_key, &true);
        env.storage().temporary().set(&bucket_key, &next_used);
        env.events().publish(
            (symbol_short!("activity"),),
            (reporter, subject, event_id, weighted),
        );
        Ok(record.activity_score)
    }

    /// Issues one credential per issuer/subject pair. Expiry is mandatory and bounded.
    pub fn issue_credential(
        env: Env,
        issuer: Address,
        subject: Address,
        kind: String,
        weight: i64,
        expires_at: u64,
    ) -> Result<(), Error> {
        Self::assert_writable(&env)?;
        issuer.require_auth();
        let config: IssuerConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Issuer(issuer.clone()))
            .ok_or(Error::IssuerNotFound)?;
        if !config.active {
            return Err(Error::IssuerNotFound);
        }
        let now = env.ledger().timestamp();
        if kind.len() == 0
            || kind.len() > 64
            || weight <= 0
            || weight > config.max_credential_weight
            || expires_at <= now
            || expires_at.saturating_sub(now) > MAX_CREDENTIAL_LIFETIME
        {
            return Err(Error::InvalidCredential);
        }
        let key = DataKey::Credential(subject.clone(), issuer.clone());
        let existing: Option<Credential> = env.storage().persistent().get(&key);
        if let Some(ref credential) = existing {
            if !credential.revoked && credential.expires_at > now {
                return Err(Error::CredentialAlreadyActive);
            }
        }
        let mut record = Self::subject(&env, &subject)?;
        if existing.is_none() {
            if record.issuers.len() >= MAX_CREDENTIALS {
                return Err(Error::CredentialLimitExceeded);
            }
            record.issuers.push_back(issuer.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Subject(subject.clone()), &record);
        }
        let credential = Credential {
            issuer: issuer.clone(),
            kind,
            weight,
            issued_at: now,
            expires_at,
            revoked: false,
        };
        env.storage().persistent().set(&key, &credential);
        env.events().publish(
            (symbol_short!("cred_add"),),
            (issuer, subject, weight, expires_at),
        );
        Ok(())
    }

    pub fn revoke_credential(env: Env, issuer: Address, subject: Address) -> Result<(), Error> {
        Self::initialized_or_error(&env)?;
        issuer.require_auth();
        let key = DataKey::Credential(subject.clone(), issuer.clone());
        let mut credential: Credential = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::CredentialNotFound)?;
        if credential.revoked {
            return Err(Error::CredentialNotFound);
        }
        credential.revoked = true;
        env.storage().persistent().set(&key, &credential);
        env.events()
            .publish((symbol_short!("cred_rev"),), (issuer, subject));
        Ok(())
    }

    pub fn get_score(env: Env, subject: Address) -> Result<ScoreMatrix, Error> {
        let record = Self::subject(&env, &subject)?;
        Self::matrix(&env, &subject, &record)
    }

    pub fn get_scores(env: Env, subjects: Vec<Address>) -> Result<Vec<ScoreMatrix>, Error> {
        if subjects.len() > MAX_BATCH {
            return Err(Error::BatchLimitExceeded);
        }
        let mut result = Vec::new(&env);
        for subject in subjects.iter() {
            let record = Self::subject(&env, &subject)?;
            result.push_back(Self::matrix(&env, &subject, &record)?);
        }
        Ok(result)
    }

    pub fn is_registered(env: Env, subject: Address) -> bool {
        env.storage().persistent().has(&DataKey::Subject(subject))
    }

    fn matrix(env: &Env, subject: &Address, record: &SubjectRecord) -> Result<ScoreMatrix, Error> {
        let mut decayed = record.clone();
        Self::apply_decay(env, &mut decayed, env.ledger().timestamp())?;
        let now = env.ledger().timestamp();
        let mut credential_score = 0i64;
        let mut active_credentials = 0u32;
        for issuer in record.issuers.iter() {
            let config: Option<IssuerConfig> = env
                .storage()
                .persistent()
                .get(&DataKey::Issuer(issuer.clone()));
            let credential: Option<Credential> = env
                .storage()
                .persistent()
                .get(&DataKey::Credential(subject.clone(), issuer));
            if let (Some(cfg), Some(cred)) = (config, credential) {
                if cfg.active && !cred.revoked && cred.expires_at > now {
                    credential_score = credential_score
                        .saturating_add(cred.weight)
                        .min(MAX_CREDENTIAL_SCORE);
                    active_credentials = active_credentials.saturating_add(1);
                }
            }
        }
        let confidence_bps = match active_credentials {
            0 => 2_500,
            1 => 6_000,
            2 => 8_000,
            _ => 10_000,
        };
        let total = decayed.activity_score.saturating_add(credential_score);
        let final_score = total.saturating_mul(confidence_bps as i64) / BPS;
        Ok(ScoreMatrix {
            activity_score: decayed.activity_score,
            credential_score,
            active_credentials,
            confidence_bps,
            final_score,
        })
    }

    fn apply_decay(env: &Env, record: &mut SubjectRecord, now: u64) -> Result<(), Error> {
        let epoch_seconds = Self::epoch_seconds(env)?;
        let epochs = now.saturating_sub(record.last_decay_at) / epoch_seconds;
        if epochs == 0 {
            return Ok(());
        }
        if epochs >= MAX_DECAY_EPOCHS {
            record.activity_score = 0;
            record.last_decay_at = now;
            return Ok(());
        }
        let retain = BPS - Self::decay_bps(env)? as i64;
        for _ in 0..epochs {
            record.activity_score = record.activity_score.saturating_mul(retain) / BPS;
            if record.activity_score == 0 {
                break;
            }
        }
        record.last_decay_at = record
            .last_decay_at
            .saturating_add(epochs.saturating_mul(epoch_seconds));
        Ok(())
    }

    fn subject(env: &Env, subject: &Address) -> Result<SubjectRecord, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Subject(subject.clone()))
            .ok_or(Error::SubjectNotFound)
    }
    fn initialized(env: &Env) -> bool {
        env.storage()
            .instance()
            .get(&InstanceKey::Initialized)
            .unwrap_or(false)
    }
    fn initialized_or_error(env: &Env) -> Result<(), Error> {
        if Self::initialized(env) {
            Ok(())
        } else {
            Err(Error::NotInitialized)
        }
    }
    fn assert_writable(env: &Env) -> Result<(), Error> {
        Self::initialized_or_error(env)?;
        if env
            .storage()
            .instance()
            .get(&InstanceKey::Paused)
            .unwrap_or(false)
        {
            Err(Error::Paused)
        } else {
            Ok(())
        }
    }
    fn admin(env: &Env) -> Result<Address, Error> {
        Self::initialized_or_error(env)?;
        env.storage()
            .instance()
            .get(&InstanceKey::Admin)
            .ok_or(Error::NotInitialized)
    }
    fn epoch_seconds(env: &Env) -> Result<u64, Error> {
        env.storage()
            .instance()
            .get(&InstanceKey::EpochSeconds)
            .ok_or(Error::NotInitialized)
    }
    fn decay_bps(env: &Env) -> Result<u32, Error> {
        env.storage()
            .instance()
            .get(&InstanceKey::DecayBps)
            .ok_or(Error::NotInitialized)
    }
}
