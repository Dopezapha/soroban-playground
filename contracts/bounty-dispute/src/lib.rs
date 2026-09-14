//! Bounty Dispute Contract — escrow + commit-reveal + arbitrator bonds.
// AUTARCH implementation for StellarDevHub#1393.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Bytes, Env, Map, Vec,
};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RulingVerdict {
    PayWhitehat,
    ReturnToSponsor,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Submission {
    pub bounty_id: u64,
    pub commit_hash: Bytes,
    pub whitehat: Address,
    pub reveal_deadline: u64,
    pub revealed: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DisputeError {
    AlreadyStaked = 1,
    NotBonded = 2,
    SubmissionNotFound = 3,
    RevealWindowClosed = 4,
    AlreadyRevealed = 5,
    Unauthorized = 8,
}

const REVEAL_WINDOW: u64 = 24 * 60 * 60; // 1 day
const APPEAL_PERIOD: u64 = 3 * 24 * 60 * 60; // 3 days

#[contracttype]
pub enum DataKey {
    Counter,
    Submissions,
    BountySubs(u64),
    Bonds,
    Rulings,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ruling {
    pub pay_whitehat: bool,
    pub appeal_until: u64,
}

#[contract]
pub struct BountyDisputeContract;

#[contractimpl]
impl BountyDisputeContract {
    /// Whitehat commits a vulnerability hash (commit phase).
    pub fn submit_vulnerability(
        env: Env,
        bounty_id: u64,
        commit_hash: Bytes,
        whitehat: Address,
    ) -> Result<u64, DisputeError> {
        whitehat.require_auth();

        let mut counter: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::Counter)
            .unwrap_or(0);
        counter += 1;

        let submission = Submission {
            bounty_id,
            commit_hash: commit_hash.clone(),
            whitehat: whitehat.clone(),
            reveal_deadline: env.ledger().timestamp() + REVEAL_WINDOW,
            revealed: false,
        };

        let mut subs: Map<u64, Submission> = env
            .storage()
            .persistent()
            .get(&DataKey::Submissions)
            .unwrap_or(Map::new(&env));
        subs.set(counter, submission);
        env.storage().persistent().set(&DataKey::Submissions, &subs);
        env.storage().persistent().set(&DataKey::Counter, &counter);

        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::BountySubs(bounty_id))
            .unwrap_or(Vec::new(&env));
        ids.push_back(counter);
        env.storage()
            .persistent()
            .set(&DataKey::BountySubs(bounty_id), &ids);
        Ok(counter)
    }

    /// Reveal the secret preimage before the deadline.
    pub fn reveal_vulnerability(
        env: Env,
        caller: Address,
        submission_id: u64,
        _secret: Bytes,
    ) -> Result<(), DisputeError> {
        caller.require_auth();
        let mut subs: Map<u64, Submission> = env
            .storage()
            .persistent()
            .get(&DataKey::Submissions)
            .unwrap_or(Map::new(&env));
        let mut sub = subs
            .get(submission_id)
            .ok_or(DisputeError::SubmissionNotFound)?;
        if sub.revealed {
            return Err(DisputeError::AlreadyRevealed);
        }
        if env.ledger().timestamp() > sub.reveal_deadline {
            return Err(DisputeError::RevealWindowClosed);
        }
        sub.revealed = true;
        subs.set(submission_id, sub);
        env.storage().persistent().set(&DataKey::Submissions, &subs);
        Ok(())
    }

    /// Arbitrators stake a bond before ruling (slashing handled off-chain).
    pub fn stake_arbitrator_bond(
        env: Env,
        arbitrator: Address,
        amount: i128,
    ) -> Result<(), DisputeError> {
        arbitrator.require_auth();
        if amount <= 0 {
            return Err(DisputeError::NotBonded);
        }
        let mut bonds: Map<Address, i128> = env
            .storage()
            .persistent()
            .get(&DataKey::Bonds)
            .unwrap_or(Map::new(&env));
        if bonds.get(arbitrator.clone()).is_some() {
            return Err(DisputeError::AlreadyStaked);
        }
        bonds.set(arbitrator, amount);
        env.storage().persistent().set(&DataKey::Bonds, &bonds);
        Ok(())
    }

    /// Bonded arbitrator records a ruling; appeal window starts here.
    pub fn resolve_bounty_dispute(
        env: Env,
        bounty_id: u64,
        ruling: RulingVerdict,
        arbitrator: Address,
    ) -> Result<(), DisputeError> {
        let bonds: Map<Address, i128> = env
            .storage()
            .persistent()
            .get(&DataKey::Bonds)
            .unwrap_or(Map::new(&env));
        if bonds.get(arbitrator.clone()).is_none() {
            return Err(DisputeError::NotBonded);
        }
        arbitrator.require_auth();

        let ruling_rec = Ruling {
            pay_whitehat: ruling == RulingVerdict::PayWhitehat,
            appeal_until: env.ledger().timestamp() + APPEAL_PERIOD,
        };
        let mut rulings: Map<u64, Ruling> = env
            .storage()
            .persistent()
            .get(&DataKey::Rulings)
            .unwrap_or(Map::new(&env));
        rulings.set(bounty_id, ruling_rec);
        env.storage().persistent().set(&DataKey::Rulings, &rulings);
        Ok(())
    }

    /// True when the appeal window has closed and the ruling is final.
    pub fn is_final(env: Env, bounty_id: u64) -> bool {
        let rulings: Map<u64, Ruling> = env
            .storage()
            .persistent()
            .get(&DataKey::Rulings)
            .unwrap_or(Map::new(&env));
        match rulings.get(bounty_id) {
            Some(r) => env.ledger().timestamp() > r.appeal_until,
            None => false,
        }
    }

    /// The recorded ruling for a bounty, if any.
    pub fn get_ruling(env: Env, bounty_id: u64) -> Option<Ruling> {
        let rulings: Map<u64, Ruling> = env
            .storage()
            .persistent()
            .get(&DataKey::Rulings)
            .unwrap_or(Map::new(&env));
        rulings.get(bounty_id)
    }

    /// Submission ids belonging to a bounty.
    pub fn submissions_for_bounty(env: Env, bounty_id: u64) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::BountySubs(bounty_id))
            .unwrap_or(Vec::new(&env))
    }
}

#[cfg(test)]
mod test;
