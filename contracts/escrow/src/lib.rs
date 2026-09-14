#![no_std]

mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Bytes,
    BytesN, Env, Vec,
};

use crate::storage::{
    get_admin, get_analytics, get_arbiter_fee_bps, get_atomic_swap, get_atomic_swap_count,
    get_atomic_swap_stats, get_escrow, get_escrow_count, get_milestone, increment_escrow_count,
    is_initialized, next_atomic_swap_id, set_admin, set_analytics, set_arbiter_fee_bps,
    set_atomic_swap, set_atomic_swap_stats, set_escrow, set_initialized, set_milestone,
};
pub use crate::types::{
    Analytics, AtomicSwap, AtomicSwapStats, AtomicSwapStatus, Error, Escrow, EscrowStatus,
    Milestone, MilestoneStatus, Ruling,
};

const MIN_TIMELOCK_SECONDS: u64 = 60;
const MAX_TIMELOCK_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_PREIMAGE_BYTES: u32 = 64;
const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
const INSTANCE_TTL_TARGET: u32 = 1_555_200;

#[contract]
pub struct FreelancerEscrow;

#[contractimpl]
impl FreelancerEscrow {
    /// Initialize the contract. `arbiter_fee_bps` is basis points (e.g. 200 = 2%).
    pub fn initialize(env: Env, admin: Address, arbiter_fee_bps: u32) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        if arbiter_fee_bps > 10_000 {
            return Err(Error::InvalidFeeBps);
        }
        set_admin(&env, &admin);
        set_arbiter_fee_bps(&env, arbiter_fee_bps);
        set_initialized(&env);
        set_analytics(
            &env,
            &Analytics {
                total_escrows: 0,
                active_escrows: 0,
                completed_escrows: 0,
                disputed_escrows: 0,
                cancelled_escrows: 0,
                total_value_locked: 0,
                total_paid_out: 0,
            },
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_TARGET);
        Ok(())
    }

    /// Lock the maker's offered asset in a bilateral SHA-256 HTLC.
    ///
    /// The designated taker must fund the requested asset before expiry. Once
    /// funded, either party or a relayer may reveal the preimage to atomically
    /// exchange both token legs.
    pub fn create_atomic_swap(
        env: Env,
        maker: Address,
        taker: Address,
        offered_token: Address,
        offered_amount: i128,
        requested_token: Address,
        requested_amount: i128,
        hashlock: BytesN<32>,
        expires_at: u64,
    ) -> Result<u64, Error> {
        ensure_initialized(&env)?;
        maker.require_auth();
        if maker == taker {
            return Err(Error::InvalidSwap);
        }
        if offered_token == requested_token {
            return Err(Error::SameAsset);
        }
        if offered_amount <= 0 || requested_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let now = env.ledger().timestamp();
        let min_expiry = now
            .checked_add(MIN_TIMELOCK_SECONDS)
            .ok_or(Error::Overflow)?;
        let max_expiry = now
            .checked_add(MAX_TIMELOCK_SECONDS)
            .ok_or(Error::Overflow)?;
        if expires_at < min_expiry || expires_at > max_expiry {
            return Err(Error::TimelockOutOfRange);
        }
        if hashlock == BytesN::from_array(&env, &[0; 32]) {
            return Err(Error::InvalidHashlock);
        }

        let id = next_atomic_swap_id(&env)?;
        let swap = AtomicSwap {
            id,
            maker: maker.clone(),
            taker: taker.clone(),
            offered_token: offered_token.clone(),
            offered_amount,
            requested_token,
            requested_amount,
            hashlock,
            expires_at,
            status: AtomicSwapStatus::AwaitingCounterparty,
            revealed_preimage: None,
            created_at: now,
            funded_at: None,
            settled_at: None,
        };
        set_atomic_swap(&env, &swap);
        let mut stats = get_atomic_swap_stats(&env);
        stats.total = stats.total.checked_add(1).ok_or(Error::Overflow)?;
        stats.active = stats.active.checked_add(1).ok_or(Error::Overflow)?;
        set_atomic_swap_stats(&env, &stats);

        token::Client::new(&env, &offered_token).transfer(
            &maker,
            &env.current_contract_address(),
            &offered_amount,
        );
        env.events()
            .publish((symbol_short!("swapopen"), id), (maker, taker));
        Ok(id)
    }

    /// Lock the requested asset. Only the designated taker can fund this leg.
    pub fn fund_atomic_swap(env: Env, swap_id: u64, taker: Address) -> Result<(), Error> {
        ensure_initialized(&env)?;
        taker.require_auth();
        let mut swap = get_atomic_swap(&env, swap_id)?;
        if swap.taker != taker {
            return Err(Error::Unauthorized);
        }
        if swap.status != AtomicSwapStatus::AwaitingCounterparty {
            return Err(Error::InvalidState);
        }
        if env.ledger().timestamp() >= swap.expires_at {
            return Err(Error::SwapExpired);
        }

        swap.status = AtomicSwapStatus::Funded;
        swap.funded_at = Some(env.ledger().timestamp());
        set_atomic_swap(&env, &swap);
        token::Client::new(&env, &swap.requested_token).transfer(
            &taker,
            &env.current_contract_address(),
            &swap.requested_amount,
        );
        env.events()
            .publish((symbol_short!("funded"), swap_id), taker);
        Ok(())
    }

    /// Reveal the preimage and atomically exchange both escrowed assets.
    /// This is permissionless so relayers and linked contracts can settle it.
    pub fn claim_atomic_swap(env: Env, swap_id: u64, preimage: Bytes) -> Result<(), Error> {
        ensure_initialized(&env)?;
        if preimage.is_empty() || preimage.len() > MAX_PREIMAGE_BYTES {
            return Err(Error::InvalidPreimage);
        }
        let mut swap = get_atomic_swap(&env, swap_id)?;
        if swap.status != AtomicSwapStatus::Funded {
            return Err(Error::InvalidState);
        }
        let now = env.ledger().timestamp();
        if now >= swap.expires_at {
            return Err(Error::SwapExpired);
        }
        let computed = env.crypto().sha256(&preimage);
        let computed_hash = BytesN::<32>::from_array(&env, &computed.to_array());
        if computed_hash != swap.hashlock {
            return Err(Error::InvalidPreimage);
        }

        swap.status = AtomicSwapStatus::Claimed;
        swap.revealed_preimage = Some(preimage.clone());
        swap.settled_at = Some(now);
        set_atomic_swap(&env, &swap);
        let mut stats = get_atomic_swap_stats(&env);
        stats.active = stats.active.checked_sub(1).ok_or(Error::Overflow)?;
        stats.claimed = stats.claimed.checked_add(1).ok_or(Error::Overflow)?;
        set_atomic_swap_stats(&env, &stats);

        token::Client::new(&env, &swap.offered_token).transfer(
            &env.current_contract_address(),
            &swap.taker,
            &swap.offered_amount,
        );
        token::Client::new(&env, &swap.requested_token).transfer(
            &env.current_contract_address(),
            &swap.maker,
            &swap.requested_amount,
        );
        env.events()
            .publish((symbol_short!("claimed"), swap_id), preimage);
        Ok(())
    }

    /// Return all funded legs after expiry. Anyone may trigger the refund.
    pub fn refund_atomic_swap(env: Env, swap_id: u64) -> Result<(), Error> {
        ensure_initialized(&env)?;
        let mut swap = get_atomic_swap(&env, swap_id)?;
        let was_funded = swap.status == AtomicSwapStatus::Funded;
        if !was_funded && swap.status != AtomicSwapStatus::AwaitingCounterparty {
            return Err(Error::InvalidState);
        }
        let now = env.ledger().timestamp();
        if now < swap.expires_at {
            return Err(Error::SwapNotExpired);
        }

        swap.status = AtomicSwapStatus::Refunded;
        swap.settled_at = Some(now);
        set_atomic_swap(&env, &swap);
        let mut stats = get_atomic_swap_stats(&env);
        stats.active = stats.active.checked_sub(1).ok_or(Error::Overflow)?;
        stats.refunded = stats.refunded.checked_add(1).ok_or(Error::Overflow)?;
        set_atomic_swap_stats(&env, &stats);

        token::Client::new(&env, &swap.offered_token).transfer(
            &env.current_contract_address(),
            &swap.maker,
            &swap.offered_amount,
        );
        if was_funded {
            token::Client::new(&env, &swap.requested_token).transfer(
                &env.current_contract_address(),
                &swap.taker,
                &swap.requested_amount,
            );
        }
        env.events()
            .publish((symbol_short!("refunded"), swap_id), ());
        Ok(())
    }

    /// Maker cancellation is allowed only before the taker funds the swap.
    pub fn cancel_atomic_swap(env: Env, swap_id: u64, maker: Address) -> Result<(), Error> {
        ensure_initialized(&env)?;
        maker.require_auth();
        let mut swap = get_atomic_swap(&env, swap_id)?;
        if swap.maker != maker {
            return Err(Error::Unauthorized);
        }
        if swap.status != AtomicSwapStatus::AwaitingCounterparty {
            return Err(Error::InvalidState);
        }
        if env.ledger().timestamp() >= swap.expires_at {
            return Err(Error::SwapExpired);
        }

        swap.status = AtomicSwapStatus::Cancelled;
        swap.settled_at = Some(env.ledger().timestamp());
        set_atomic_swap(&env, &swap);
        let mut stats = get_atomic_swap_stats(&env);
        stats.active = stats.active.checked_sub(1).ok_or(Error::Overflow)?;
        stats.cancelled = stats.cancelled.checked_add(1).ok_or(Error::Overflow)?;
        set_atomic_swap_stats(&env, &stats);
        token::Client::new(&env, &swap.offered_token).transfer(
            &env.current_contract_address(),
            &maker,
            &swap.offered_amount,
        );
        env.events()
            .publish((symbol_short!("cancelled"), swap_id), maker);
        Ok(())
    }

    /// Client creates a new escrow agreement.
    /// `milestone_amounts` must be non-empty and sum to `total_amount`.
    pub fn create_escrow(
        env: Env,
        client: Address,
        freelancer: Address,
        arbiter: Address,
        total_amount: i128,
        milestone_amounts: Vec<i128>,
    ) -> Result<u32, Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        if total_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if milestone_amounts.is_empty() {
            return Err(Error::NoMilestones);
        }
        if milestone_amounts.len() > 20 {
            return Err(Error::TooManyMilestones);
        }

        // Validate milestones sum to total_amount.
        let mut sum: i128 = 0;
        for amt in milestone_amounts.iter() {
            if amt <= 0 {
                return Err(Error::InvalidAmount);
            }
            sum += amt;
        }
        if sum != total_amount {
            return Err(Error::InvalidAmount);
        }

        let id = increment_escrow_count(&env);
        let arbiter_fee_bps = get_arbiter_fee_bps(&env);

        let escrow = Escrow {
            id,
            client: client.clone(),
            freelancer: freelancer.clone(),
            arbiter,
            total_amount,
            paid_amount: 0,
            milestone_count: milestone_amounts.len(),
            status: EscrowStatus::Pending,
            created_at: env.ledger().timestamp(),
            arbiter_fee_bps,
        };
        set_escrow(&env, &escrow);

        // Persist each milestone.
        let mut milestone_id: u32 = 1;
        for amt in milestone_amounts.iter() {
            set_milestone(
                &env,
                id,
                &Milestone {
                    id: milestone_id,
                    amount: amt,
                    status: MilestoneStatus::Pending,
                },
            );
            milestone_id += 1;
        }

        let mut analytics = get_analytics(&env);
        analytics.total_escrows += 1;
        set_analytics(&env, &analytics);

        Ok(id)
    }

    /// Client deposits the full `total_amount`, activating the escrow.
    pub fn deposit(env: Env, escrow_id: u32, client: Address) -> Result<(), Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        let mut escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != client {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Pending {
            return Err(Error::InvalidState);
        }

        escrow.status = EscrowStatus::Active;
        // Mark first milestone as InProgress automatically.
        let mut first = get_milestone(&env, escrow_id, 1)?;
        first.status = MilestoneStatus::InProgress;
        set_milestone(&env, escrow_id, &first);
        set_escrow(&env, &escrow);

        let mut analytics = get_analytics(&env);
        analytics.active_escrows += 1;
        analytics.total_value_locked += escrow.total_amount;
        set_analytics(&env, &analytics);

        Ok(())
    }

    /// Freelancer submits a milestone for client review.
    pub fn submit_milestone(
        env: Env,
        escrow_id: u32,
        freelancer: Address,
        milestone_id: u32,
    ) -> Result<(), Error> {
        ensure_initialized(&env)?;
        freelancer.require_auth();

        let escrow = get_escrow(&env, escrow_id)?;
        if escrow.freelancer != freelancer {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Active {
            return Err(Error::InvalidState);
        }

        let mut milestone = get_milestone(&env, escrow_id, milestone_id)?;
        if milestone.status != MilestoneStatus::InProgress {
            return Err(Error::InvalidState);
        }

        milestone.status = MilestoneStatus::UnderReview;
        set_milestone(&env, escrow_id, &milestone);

        Ok(())
    }

    /// Client approves a submitted milestone.
    pub fn approve_milestone(
        env: Env,
        escrow_id: u32,
        client: Address,
        milestone_id: u32,
    ) -> Result<(), Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        let escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != client {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Active {
            return Err(Error::InvalidState);
        }

        let mut milestone = get_milestone(&env, escrow_id, milestone_id)?;
        if milestone.status != MilestoneStatus::UnderReview {
            return Err(Error::InvalidState);
        }

        milestone.status = MilestoneStatus::Approved;
        set_milestone(&env, escrow_id, &milestone);

        Ok(())
    }

    /// Client rejects a submitted milestone, sending it back to InProgress.
    pub fn reject_milestone(
        env: Env,
        escrow_id: u32,
        client: Address,
        milestone_id: u32,
    ) -> Result<(), Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        let escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != client {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Active {
            return Err(Error::InvalidState);
        }

        let mut milestone = get_milestone(&env, escrow_id, milestone_id)?;
        if milestone.status != MilestoneStatus::UnderReview {
            return Err(Error::InvalidState);
        }

        milestone.status = MilestoneStatus::InProgress;
        set_milestone(&env, escrow_id, &milestone);

        Ok(())
    }

    /// Client releases payment for an approved milestone.
    /// Returns the net amount paid to the freelancer (after arbiter fee).
    pub fn release_payment(
        env: Env,
        escrow_id: u32,
        client: Address,
        milestone_id: u32,
    ) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        let mut escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != client {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Active {
            return Err(Error::InvalidState);
        }

        let mut milestone = get_milestone(&env, escrow_id, milestone_id)?;
        if milestone.status != MilestoneStatus::Approved {
            return Err(Error::InvalidState);
        }

        let fee = (milestone.amount * escrow.arbiter_fee_bps as i128) / 10_000;
        let net = milestone.amount - fee;

        milestone.status = MilestoneStatus::Paid;
        set_milestone(&env, escrow_id, &milestone);

        escrow.paid_amount += milestone.amount;

        // Start the next pending milestone automatically.
        let next_id = milestone_id + 1;
        if next_id <= escrow.milestone_count {
            if let Ok(mut next) = get_milestone(&env, escrow_id, next_id) {
                if next.status == MilestoneStatus::Pending {
                    next.status = MilestoneStatus::InProgress;
                    set_milestone(&env, escrow_id, &next);
                }
            }
        }

        // If all milestones paid, complete the escrow.
        if escrow.paid_amount >= escrow.total_amount {
            escrow.status = EscrowStatus::Completed;
            let mut analytics = get_analytics(&env);
            analytics.active_escrows = analytics.active_escrows.saturating_sub(1);
            analytics.completed_escrows += 1;
            analytics.total_value_locked = analytics
                .total_value_locked
                .saturating_sub(escrow.total_amount);
            analytics.total_paid_out += escrow.total_amount;
            set_analytics(&env, &analytics);
        }

        set_escrow(&env, &escrow);

        Ok(net)
    }

    /// Either party raises a dispute, locking the escrow for arbiter review.
    pub fn raise_dispute(env: Env, escrow_id: u32, initiator: Address) -> Result<(), Error> {
        ensure_initialized(&env)?;
        initiator.require_auth();

        let mut escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != initiator && escrow.freelancer != initiator {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Active {
            return Err(Error::InvalidState);
        }

        escrow.status = EscrowStatus::Disputed;
        set_escrow(&env, &escrow);

        let mut analytics = get_analytics(&env);
        analytics.disputed_escrows += 1;
        analytics.active_escrows = analytics.active_escrows.saturating_sub(1);
        set_analytics(&env, &analytics);

        Ok(())
    }

    /// Arbiter resolves a dispute.
    /// Ruling: 0 = FreelancerFavored, 1 = ClientFavored, 2 = Split.
    pub fn resolve_dispute(
        env: Env,
        escrow_id: u32,
        arbiter: Address,
        ruling: u32,
    ) -> Result<i128, Error> {
        ensure_initialized(&env)?;
        arbiter.require_auth();

        let mut escrow = get_escrow(&env, escrow_id)?;
        if escrow.arbiter != arbiter {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Disputed {
            return Err(Error::InvalidState);
        }

        let ruling_enum = match ruling {
            0 => Ruling::FreelancerFavored,
            1 => Ruling::ClientFavored,
            2 => Ruling::Split,
            _ => return Err(Error::InvalidRuling),
        };

        let remaining = escrow.total_amount - escrow.paid_amount;
        let fee = (remaining * escrow.arbiter_fee_bps as i128) / 10_000;

        let payout = match ruling_enum {
            Ruling::FreelancerFavored => remaining - fee,
            Ruling::ClientFavored => 0,
            Ruling::Split => (remaining - fee) / 2,
        };

        escrow.paid_amount = escrow.total_amount; // mark fully settled
        escrow.status = EscrowStatus::Completed;
        set_escrow(&env, &escrow);

        let mut analytics = get_analytics(&env);
        analytics.completed_escrows += 1;
        analytics.total_value_locked = analytics.total_value_locked.saturating_sub(remaining);
        analytics.total_paid_out += remaining;
        set_analytics(&env, &analytics);

        Ok(payout)
    }

    /// Client cancels an escrow that has not yet been activated (Pending).
    pub fn cancel_escrow(env: Env, escrow_id: u32, client: Address) -> Result<(), Error> {
        ensure_initialized(&env)?;
        client.require_auth();

        let mut escrow = get_escrow(&env, escrow_id)?;
        if escrow.client != client {
            return Err(Error::Unauthorized);
        }
        if escrow.status != EscrowStatus::Pending {
            return Err(Error::InvalidState);
        }

        escrow.status = EscrowStatus::Cancelled;
        set_escrow(&env, &escrow);

        let mut analytics = get_analytics(&env);
        analytics.cancelled_escrows += 1;
        set_analytics(&env, &analytics);

        Ok(())
    }

    // ── Read-only queries ──────────────────────────────────────────────────────

    pub fn get_escrow(env: Env, escrow_id: u32) -> Result<Escrow, Error> {
        get_escrow(&env, escrow_id)
    }

    pub fn get_milestone(env: Env, escrow_id: u32, milestone_id: u32) -> Result<Milestone, Error> {
        get_milestone(&env, escrow_id, milestone_id)
    }

    pub fn get_analytics(env: Env) -> Result<Analytics, Error> {
        ensure_initialized(&env)?;
        Ok(get_analytics(&env))
    }

    pub fn get_escrow_count(env: Env) -> u32 {
        get_escrow_count(&env)
    }

    pub fn is_initialized(env: Env) -> bool {
        is_initialized(&env)
    }

    pub fn get_admin(env: Env) -> Result<Address, Error> {
        get_admin(&env)
    }

    pub fn get_atomic_swap(env: Env, swap_id: u64) -> Result<AtomicSwap, Error> {
        get_atomic_swap(&env, swap_id)
    }

    pub fn get_atomic_swap_count(env: Env) -> u64 {
        get_atomic_swap_count(&env)
    }

    pub fn get_atomic_swap_stats(env: Env) -> Result<AtomicSwapStats, Error> {
        ensure_initialized(&env)?;
        Ok(get_atomic_swap_stats(&env))
    }
}

fn ensure_initialized(env: &Env) -> Result<(), Error> {
    if !is_initialized(env) {
        Err(Error::NotInitialized)
    } else {
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_TARGET);
        Ok(())
    }
}
