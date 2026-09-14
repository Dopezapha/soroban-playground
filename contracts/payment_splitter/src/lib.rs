#![no_std]

//! Payment Splitter — splits a payment equally among a set of recipients.
//!
//! ## Error handling (issue #999)
//!
//! The previous version declared a `SplitterError` enum that could never leave
//! the contract: it was a plain Rust enum rather than `#[contracterror]`, so it
//! had no `u32` discriminant to encode into a failed invocation's result. A
//! caller could not distinguish "amount was zero" from "recipient list was
//! empty" — or from the contract panicking outright.
//!
//! Every failure mode is now a numbered `Error` variant, matching the
//! convention used across the other contracts in this repo, so off-chain
//! tooling can decode a failure into a specific cause.
//!
//! ## Conservation of value
//!
//! `amount` rarely divides evenly by the recipient count. The previous
//! implementation computed the remainder, wrote a comment saying it would
//! "still proceed", and dropped it — silently losing up to `n - 1` stroops on
//! every call. Splitting money must not lose money, so the remainder is now
//! distributed deterministically: the first `remainder` recipients each receive
//! one extra unit. The sum of all shares is always exactly `amount`, which
//! `split_shares` asserts.

use soroban_sdk::{contract, contracterror, contractimpl, token, Address, Env, Vec};

/// Upper bound on recipients per call.
///
/// Each recipient costs a token transfer, and a list long enough to exhaust the
/// ledger's CPU budget would fail *after* transferring to some of them —
/// leaving a partial split with no record of where it stopped. Rejecting the
/// call up front keeps the operation all-or-nothing.
pub const MAX_RECIPIENTS: u32 = 100;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// `amount` was zero or negative.
    ZeroAmount = 1,
    /// The recipient list was empty.
    NoRecipients = 2,
    /// The recipient list exceeded `MAX_RECIPIENTS`.
    TooManyRecipients = 3,
    /// The same address appeared more than once in the recipient list.
    DuplicateRecipient = 4,
    /// `amount` is smaller than the recipient count, so at least one recipient
    /// would receive nothing.
    AmountTooSmall = 5,
    /// The payer is also listed as a recipient, which would make the transfer
    /// partly a self-payment and misreport what each party received.
    PayerIsRecipient = 6,
}

#[contract]
pub struct PaymentSplitter;

#[contractimpl]
impl PaymentSplitter {
    /// Splits `amount` of `token_id` from `from` equally among `recipients`.
    ///
    /// The payer must authorise the call. Every recipient receives
    /// `amount / n`, and the first `amount % n` recipients receive one
    /// additional unit so that the distributed total is exactly `amount`.
    ///
    /// Validation runs before any transfer, so a rejected call moves no funds
    /// at all rather than failing partway through.
    pub fn split(
        env: Env,
        from: Address,
        token_id: Address,
        amount: i128,
        recipients: Vec<Address>,
    ) -> Result<(), Error> {
        let shares = Self::split_shares(&env, amount, &recipients)?;

        // Checked after the cheap validation, since it is the most expensive
        // check in the list.
        if recipients.contains(&from) {
            return Err(Error::PayerIsRecipient);
        }

        from.require_auth();

        let client = token::Client::new(&env, &token_id);
        for (index, recipient) in recipients.iter().enumerate() {
            let share = shares.get(index as u32).unwrap_or(0);
            client.transfer(&from, &recipient, &share);
        }

        Ok(())
    }

    /// Computes each recipient's share without moving any funds.
    ///
    /// Exposed so a caller can preview a split — and so the distribution logic
    /// is testable without a token contract. The returned vector always sums to
    /// exactly `amount`.
    pub fn preview_split(
        env: Env,
        amount: i128,
        recipients: Vec<Address>,
    ) -> Result<Vec<i128>, Error> {
        Self::split_shares(&env, amount, &recipients)
    }

    /// Validates the inputs and computes the per-recipient shares.
    ///
    /// The single place split arithmetic happens, so `split` and
    /// `preview_split` can never disagree about who gets what.
    fn split_shares(
        env: &Env,
        amount: i128,
        recipients: &Vec<Address>,
    ) -> Result<Vec<i128>, Error> {
        if amount <= 0 {
            return Err(Error::ZeroAmount);
        }

        let count = recipients.len();
        if count == 0 {
            return Err(Error::NoRecipients);
        }
        if count > MAX_RECIPIENTS {
            return Err(Error::TooManyRecipients);
        }

        // A duplicate would receive two shares while the caller believes it
        // received one, so the split would not match what the caller asked for.
        // O(n^2), which is acceptable only because MAX_RECIPIENTS is bounded.
        for i in 0..count {
            let current = recipients.get(i).unwrap();
            for j in (i + 1)..count {
                if current == recipients.get(j).unwrap() {
                    return Err(Error::DuplicateRecipient);
                }
            }
        }

        let count_i128 = count as i128;
        if amount < count_i128 {
            // Integer division would give at least one recipient zero. Better
            // to reject than to transfer nothing and report success.
            return Err(Error::AmountTooSmall);
        }

        let base = amount / count_i128;
        let remainder = amount % count_i128;

        // The remainder is distributed one unit at a time to the first
        // `remainder` recipients rather than discarded. Deterministic, so the
        // same inputs always produce the same allocation.
        let mut shares = Vec::new(env);
        for i in 0..count {
            let extra = if (i as i128) < remainder { 1 } else { 0 };
            shares.push_back(base + extra);
        }

        debug_assert_eq!(
            shares.iter().sum::<i128>(),
            amount,
            "split must distribute the full amount"
        );

        Ok(shares)
    }
}

#[cfg(test)]
mod test;
