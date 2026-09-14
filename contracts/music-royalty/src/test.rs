// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

#![cfg(test)]

//! Tests for the Royalty Distributor refactor (issue #1001).
//!
//! The contract had no tests at all, so the refactor had nothing holding it in
//! place. These pin the behaviour that changed — specific error variants,
//! duplicate-account rejection, checked accumulators — plus the happy paths
//! they had to keep working.

use super::*;
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, vec, Env, String as SdkString};

const HOUR: u64 = 3_600;

fn setup() -> (Env, MusicRoyaltyClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, MusicRoyalty);
    let client = MusicRoyaltyClient::new(&env, &id);
    client.initialize();
    let artist = Address::generate(&env);
    (env, client, artist)
}

fn even_splits(env: &Env, n: u32) -> Vec<Split> {
    let mut splits = Vec::new(env);
    let share = TOTAL_SHARE_BASIS_POINTS / n;
    let mut allocated = 0;
    for i in 0..n {
        // Last split absorbs the rounding so the table still totals 10000.
        let s = if i == n - 1 {
            TOTAL_SHARE_BASIS_POINTS - allocated
        } else {
            share
        };
        allocated += s;
        splits.push_back(Split {
            account: Address::generate(env),
            share: s,
        });
    }
    splits
}

fn register(client: &MusicRoyaltyClient, env: &Env, artist: &Address, id: &str) {
    client.register_song(
        artist,
        &SdkString::from_str(env, id),
        &SdkString::from_str(env, "Title"),
        &even_splits(env, 2),
    );
}

// ── Initialization ────────────────────────────────────────────────────────────

#[test]
fn initialize_is_once_only() {
    let (_env, client, _artist) = setup();
    assert_eq!(
        client.try_initialize().unwrap_err().unwrap(),
        Error::AlreadyInitialized
    );
}

// ── Split validation ──────────────────────────────────────────────────────────

#[test]
fn registers_song_with_valid_splits() {
    let (env, client, artist) = setup();
    register(&client, &env, &artist, "song-1");

    let song = client.get_song_info(&SdkString::from_str(&env, "song-1"));
    assert_eq!(song.total_royalty_earned, 0);
    assert_eq!(song.splits.len(), 2);
}

#[test]
fn rejects_empty_split_table() {
    let (env, client, artist) = setup();
    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, "s"),
                &SdkString::from_str(&env, "T"),
                &Vec::new(&env),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidSplits
    );
}

#[test]
fn rejects_splits_that_do_not_total_100_percent() {
    let (env, client, artist) = setup();
    let splits = vec![
        &env,
        Split {
            account: Address::generate(&env),
            share: 5_000,
        },
        Split {
            account: Address::generate(&env),
            share: 4_000,
        },
    ];

    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, "s"),
                &SdkString::from_str(&env, "T"),
                &splits,
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidSplits
    );
}

#[test]
fn rejects_duplicate_split_accounts() {
    let (env, client, artist) = setup();
    let repeated = Address::generate(&env);
    let splits = vec![
        &env,
        Split {
            account: repeated.clone(),
            share: 5_000,
        },
        Split {
            account: repeated,
            share: 5_000,
        },
    ];

    // The table totals 10000 and every share is in range, so this was accepted
    // before the refactor — the repeated account collected both shares while
    // the artist believed it held one.
    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, "s"),
                &SdkString::from_str(&env, "T"),
                &splits,
            )
            .unwrap_err()
            .unwrap(),
        Error::DuplicateSplitAccount
    );
}

#[test]
fn rejects_zero_share_split() {
    let (env, client, artist) = setup();
    let splits = vec![
        &env,
        Split {
            account: Address::generate(&env),
            share: 10_000,
        },
        Split {
            account: Address::generate(&env),
            share: 0,
        },
    ];

    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, "s"),
                &SdkString::from_str(&env, "T"),
                &splits,
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidSplits
    );
}

// ── Specific error variants (the core of #1001) ───────────────────────────────

#[test]
fn empty_song_id_reports_invalid_song_id_not_invalid_splits() {
    let (env, client, artist) = setup();
    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, ""),
                &SdkString::from_str(&env, "T"),
                &even_splits(&env, 2),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidSongId,
        "a bad song id must not be reported as a split problem"
    );
}

#[test]
fn empty_title_reports_invalid_title_not_invalid_splits() {
    let (env, client, artist) = setup();
    assert_eq!(
        client
            .try_register_song(
                &artist,
                &SdkString::from_str(&env, "s"),
                &SdkString::from_str(&env, ""),
                &even_splits(&env, 2),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidTitle
    );
}

#[test]
fn bad_royalty_rate_reports_invalid_royalty_rate() {
    let (env, client, artist) = setup();
    register(&client, &env, &artist, "s");

    assert_eq!(
        client
            .try_issue_license(
                &artist,
                &SdkString::from_str(&env, "s"),
                &Address::generate(&env),
                &SdkString::from_str(&env, "streaming"),
                &(MAX_ROYALTY_RATE + 1),
                &(24 * HOUR),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidRoyaltyRate
    );
}

#[test]
fn bad_duration_reports_invalid_duration() {
    let (env, client, artist) = setup();
    register(&client, &env, &artist, "s");

    assert_eq!(
        client
            .try_issue_license(
                &artist,
                &SdkString::from_str(&env, "s"),
                &Address::generate(&env),
                &SdkString::from_str(&env, "streaming"),
                &500,
                &(MIN_LICENSE_DURATION - 1),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidDuration
    );
}

#[test]
fn empty_license_type_reports_invalid_license_type() {
    let (env, client, artist) = setup();
    register(&client, &env, &artist, "s");

    assert_eq!(
        client
            .try_issue_license(
                &artist,
                &SdkString::from_str(&env, "s"),
                &Address::generate(&env),
                &SdkString::from_str(&env, ""),
                &500,
                &(24 * HOUR),
            )
            .unwrap_err()
            .unwrap(),
        Error::InvalidLicenseType
    );
}

#[test]
fn missing_license_reports_license_not_found_not_song_not_found() {
    let (env, client, artist) = setup();
    register(&client, &env, &artist, "s");

    // The song exists; only the license is missing. Reporting SongNotFound sent
    // callers looking in the wrong place.
    assert_eq!(
        client
            .try_record_usage(
                &SdkString::from_str(&env, "s"),
                &Address::generate(&env),
                &1,
                &100,
            )
            .unwrap_err()
            .unwrap(),
        Error::LicenseNotFound
    );
}

// ── Licensing and usage ───────────────────────────────────────────────────────

#[test]
fn records_usage_and_accumulates_revenue() {
    let (env, client, artist) = setup();
    let licensee = Address::generate(&env);
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");

    client.issue_license(
        &artist,
        &song_id,
        &licensee,
        &SdkString::from_str(&env, "streaming"),
        &500,
        &(24 * HOUR),
    );

    client.record_usage(&song_id, &licensee, &3, &1_000);
    client.record_usage(&song_id, &licensee, &2, &500);

    let stats = client.get_usage_stats(&song_id, &licensee);
    assert_eq!(stats.usage_count, 5);
    assert_eq!(stats.total_paid, 1_500);

    let revenue = client.get_revenue_info(&song_id);
    assert_eq!(revenue.total_revenue, 1_500);
    assert_eq!(revenue.pending_distribution, 1_500);
}

#[test]
fn rejects_usage_on_expired_license() {
    let (env, client, artist) = setup();
    let licensee = Address::generate(&env);
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");

    client.issue_license(
        &artist,
        &song_id,
        &licensee,
        &SdkString::from_str(&env, "streaming"),
        &500,
        &(2 * HOUR),
    );

    env.ledger().with_mut(|l| l.timestamp += 3 * HOUR);

    assert_eq!(
        client
            .try_record_usage(&song_id, &licensee, &1, &100)
            .unwrap_err()
            .unwrap(),
        Error::Unauthorized
    );
}

#[test]
fn rejects_zero_usage_or_payment() {
    let (env, client, artist) = setup();
    let licensee = Address::generate(&env);
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");
    client.issue_license(
        &artist,
        &song_id,
        &licensee,
        &SdkString::from_str(&env, "streaming"),
        &500,
        &(24 * HOUR),
    );

    assert_eq!(
        client
            .try_record_usage(&song_id, &licensee, &0, &100)
            .unwrap_err()
            .unwrap(),
        Error::ZeroAmount
    );
    assert_eq!(
        client
            .try_record_usage(&song_id, &licensee, &1, &0)
            .unwrap_err()
            .unwrap(),
        Error::ZeroAmount
    );
}

// ── Distribution ──────────────────────────────────────────────────────────────

#[test]
fn distributes_pending_revenue_and_resets_it() {
    let (env, client, artist) = setup();
    let licensee = Address::generate(&env);
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");
    client.issue_license(
        &artist,
        &song_id,
        &licensee,
        &SdkString::from_str(&env, "streaming"),
        &500,
        &(24 * HOUR),
    );
    client.record_usage(&song_id, &licensee, &1, &2_000);

    assert_eq!(client.distribute_royalties(&song_id), 2_000);

    let revenue = client.get_revenue_info(&song_id);
    assert_eq!(revenue.distributed_revenue, 2_000);
    assert_eq!(revenue.pending_distribution, 0, "pending must be cleared");
}

#[test]
fn rejects_distribution_with_nothing_pending() {
    let (env, client, artist) = setup();
    let licensee = Address::generate(&env);
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");
    client.issue_license(
        &artist,
        &song_id,
        &licensee,
        &SdkString::from_str(&env, "streaming"),
        &500,
        &(24 * HOUR),
    );

    // Distributing twice in a row must not pay out an empty balance.
    assert_eq!(
        client
            .try_distribute_royalties(&song_id)
            .unwrap_err()
            .unwrap(),
        Error::ZeroAmount
    );
}

#[test]
fn distribute_royalty_accumulates_lifetime_total() {
    let (env, client, artist) = setup();
    let song_id = SdkString::from_str(&env, "s");
    register(&client, &env, &artist, "s");

    client.distribute_royalty(&song_id, &1_000);
    client.distribute_royalty(&song_id, &500);

    assert_eq!(client.get_song_info(&song_id).total_royalty_earned, 1_500);
}

#[test]
fn rejects_unknown_song() {
    let (env, client, _artist) = setup();
    let missing = SdkString::from_str(&env, "nope");

    assert_eq!(
        client
            .try_distribute_royalty(&missing, &100)
            .unwrap_err()
            .unwrap(),
        Error::SongNotFound
    );
    assert_eq!(
        client.try_get_song_info(&missing).unwrap_err().unwrap(),
        Error::SongNotFound
    );
}
