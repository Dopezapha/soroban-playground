// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! Comprehensive test suite for the Emergency Pause contract.
//!
//! Covers: initialization, guardian management, threshold, proposals,
//! multi-sig execution, error codes, and full lifecycle cycles.

#![cfg(test)]

use soroban_sdk::{testutils::Address as _, Address, Env, String};

use crate::types::{Error, PauseAction};
use crate::{EmergencyPause, EmergencyPauseClient};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Deploy and initialize in one step; returns (env, admin, client).
fn setup() -> (Env, Address, EmergencyPauseClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(EmergencyPause, ());
    let client = EmergencyPauseClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &2);
    let env = std::boxed::Box::leak(std::boxed::Box::new(env));
    let client = EmergencyPauseClient::new(env, &id);
    (env.clone(), admin, client)
}

/// Setup with custom threshold.
fn setup_with_threshold(threshold: u32) -> (Env, Address, EmergencyPauseClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(EmergencyPause, ());
    let client = EmergencyPauseClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &threshold);
    let env = std::boxed::Box::leak(std::boxed::Box::new(env));
    let client = EmergencyPauseClient::new(env, &id);
    (env.clone(), admin, client)
}

/// Advance ledger timestamp by `delta` seconds.
fn advance_time(env: &Env, delta: u64) {
    env.ledger().with_mut(|l| l.timestamp += delta);
}

fn make_str(env: &Env, s: &str) -> String {
    String::from_str(env, s)
}

// ── init ──────────────────────────────────────────────────────────────────────

#[test]
fn init_sets_admin() {
    let (_, admin, client) = setup();
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn init_starts_unpaused() {
    let (_, _, client) = setup();
    assert!(!client.paused());
}

#[test]
fn init_sets_threshold() {
    let (_, _, client) = setup();
    assert_eq!(client.get_threshold(), Ok(2));
}

#[test]
fn init_with_threshold_1() {
    let (_, _, client) = setup_with_threshold(1);
    assert_eq!(client.get_threshold(), Ok(1));
}

#[test]
fn init_with_zero_threshold_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(EmergencyPause, ());
    let client = EmergencyPauseClient::new(&env, &id);
    let admin = Address::generate(&env);
    assert_eq!(
        client.try_initialize(&admin, &0),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
#[should_panic(expected = "already initialized")]
fn init_twice_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(EmergencyPause, ());
    let client = EmergencyPauseClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &1);
    client.initialize(&admin, &1);
}

// ── Guardian management ───────────────────────────────────────────────────────

#[test]
fn add_guardian_works() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    assert!(client.is_guardian(&guardian));
    assert_eq!(client.guardian_count(), 1);
}

#[test]
fn add_guardian_duplicate_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    assert_eq!(
        client.try_add_guardian(&admin, &guardian),
        Err(Ok(Error::GuardianAlreadyAdded))
    );
}

#[test]
fn add_guardian_by_non_admin_fails() {
    let (env, _, client) = setup();
    let guardian = Address::generate(&env);
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_add_guardian(&stranger, &guardian),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn remove_guardian_works() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    assert!(client.is_guardian(&guardian));
    client.remove_guardian(&admin, &guardian);
    assert!(!client.is_guardian(&guardian));
    assert_eq!(client.guardian_count(), 0);
}

#[test]
fn remove_guardian_not_found_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    assert_eq!(
        client.try_remove_guardian(&admin, &guardian),
        Err(Ok(Error::GuardianNotFound))
    );
}

// ── Threshold ─────────────────────────────────────────────────────────────────

#[test]
fn set_threshold_works() {
    let (env, admin, client) = setup();
    client.set_threshold(&admin, &3);
    assert_eq!(client.get_threshold(), Ok(3));
}

#[test]
fn set_threshold_zero_fails() {
    let (env, admin, client) = setup();
    assert_eq!(
        client.try_set_threshold(&admin, &0),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn set_threshold_too_high_fails() {
    let (env, admin, client) = setup();
    // Default threshold is 2, guardian count is 0
    // Max threshold = guardian_count + 1 = 1
    assert_eq!(
        client.try_set_threshold(&admin, &5),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn set_threshold_by_non_admin_fails() {
    let (env, _, client) = setup();
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_set_threshold(&stranger, &1),
        Err(Ok(Error::Unauthorized))
    );
}

// ── Proposal lifecycle ────────────────────────────────────────────────────────

#[test]
fn create_proposal_works() {
    let (env, admin, client) = setup();
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "security incident"),
        &3600,
    );
    assert_eq!(id, 1);
    assert_eq!(client.proposal_count(), 1);
    let proposal = client.get_proposal(&id);
    assert_eq!(proposal.proposer, admin);
    assert_eq!(proposal.action, PauseAction::Pause);
    assert!(!proposal.executed);
}

#[test]
fn create_proposal_by_non_guardian_works() {
    let (env, _, client) = setup();
    let rando = Address::generate(&env);
    let id = client.create_proposal(
        &rando,
        &PauseAction::Pause,
        &make_str(&env, "request"),
        &3600,
    );
    assert_eq!(id, 1);
}

#[test]
fn create_proposal_empty_reason_works() {
    let (env, admin, client) = setup();
    let id = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, ""), &3600);
    assert_eq!(id, 1);
}

#[test]
fn create_proposal_on_uninitialized_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(EmergencyPause, ());
    let client = EmergencyPauseClient::new(&env, &id);
    let rando = Address::generate(&env);
    assert_eq!(
        client.try_create_proposal(
            &rando,
            &PauseAction::Pause,
            &make_str(&env, "reason"),
            &3600,
        ),
        Err(Ok(Error::NotInitialized))
    );
}

// ── Sign proposal ─────────────────────────────────────────────────────────────

#[test]
fn sign_proposal_works() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    let proposal = client.get_proposal(&id);
    assert_eq!(proposal.signers.len(), 1);
}

#[test]
fn sign_proposal_by_non_guardian_fails() {
    let (env, admin, client) = setup();
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_sign_proposal(&stranger, &id),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn sign_proposal_already_signed_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    assert_eq!(
        client.try_sign_proposal(&guardian, &id),
        Err(Ok(Error::AlreadyExists))
    );
}

#[test]
fn sign_proposal_expired_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "reason"), &100);
    advance_time(&env, 200);
    assert_eq!(
        client.try_sign_proposal(&guardian, &id),
        Err(Ok(Error::ProposalExpired))
    );
}

#[test]
fn sign_proposal_executed_fails() {
    let (env, admin, client) = setup();
    let guardian1 = Address::generate(&env);
    let guardian2 = Address::generate(&env);
    client.add_guardian(&admin, &guardian1);
    client.add_guardian(&admin, &guardian2);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian1, &id);
    client.sign_proposal(&guardian2, &id);
    client.execute_proposal(&admin, &id);
    assert_eq!(
        client.try_sign_proposal(&guardian1, &id),
        Err(Ok(Error::ProposalAlreadyExecuted))
    );
}

// ── Execute proposal ──────────────────────────────────────────────────────────

#[test]
fn execute_proposal_pause_works() {
    let (env, admin, client) = setup();
    let guardian1 = Address::generate(&env);
    let guardian2 = Address::generate(&env);
    client.add_guardian(&admin, &guardian1);
    client.add_guardian(&admin, &guardian2);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "emergency"),
        &3600,
    );
    client.sign_proposal(&guardian1, &id);
    client.sign_proposal(&guardian2, &id);
    client.execute_proposal(&admin, &id);
    assert!(client.paused());
    assert_eq!(client.get_pause_reason(), Some(make_str(&env, "emergency")));
    assert!(client.get_pause_timestamp().is_some());
}

#[test]
fn execute_proposal_unpause_works() {
    let (env, admin, client) = setup();
    let guardian1 = Address::generate(&env);
    let guardian2 = Address::generate(&env);
    client.add_guardian(&admin, &guardian1);
    client.add_guardian(&admin, &guardian2);

    // Pause first
    let id1 = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "pause"), &3600);
    client.sign_proposal(&guardian1, &id1);
    client.sign_proposal(&guardian2, &id1);
    client.execute_proposal(&admin, &id1);
    assert!(client.paused());

    // Unpause
    let id2 = client.create_proposal(
        &admin,
        &PauseAction::Unpause,
        &make_str(&env, "resume"),
        &3600,
    );
    client.sign_proposal(&guardian1, &id2);
    client.sign_proposal(&guardian2, &id2);
    client.execute_proposal(&admin, &id2);
    assert!(!client.paused());
}

#[test]
fn execute_proposal_insufficient_signatures_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    assert_eq!(
        client.try_execute_proposal(&admin, &id),
        Err(Ok(Error::InsufficientSignatures))
    );
}

#[test]
fn execute_proposal_expired_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "reason"), &100);
    advance_time(&env, 200);
    assert_eq!(
        client.try_execute_proposal(&admin, &id),
        Err(Ok(Error::ProposalExpired))
    );
}

#[test]
fn execute_proposal_already_executed_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);
    assert_eq!(
        client.try_execute_proposal(&admin, &id),
        Err(Ok(Error::ProposalAlreadyExecuted))
    );
}

#[test]
fn execute_proposal_not_found_fails() {
    let (env, admin, client) = setup();
    assert_eq!(
        client.try_execute_proposal(&admin, &999),
        Err(Ok(Error::ProposalNotFound))
    );
}

#[test]
fn execute_pause_when_already_paused_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "first"), &3600);
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);

    let id2 = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "second"),
        &3600,
    );
    client.sign_proposal(&guardian, &id2);
    assert_eq!(
        client.try_execute_proposal(&admin, &id2),
        Err(Ok(Error::AlreadyInState))
    );
}

#[test]
fn execute_unpause_when_not_paused_fails() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Unpause,
        &make_str(&env, "resume"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    assert_eq!(
        client.try_execute_proposal(&admin, &id),
        Err(Ok(Error::AlreadyInState))
    );
}

// ── Queries ───────────────────────────────────────────────────────────────────

#[test]
fn get_pause_reason_none_when_not_paused() {
    let (_, _, client) = setup();
    assert!(client.get_pause_reason().is_none());
}

#[test]
fn get_pause_timestamp_none_when_not_paused() {
    let (_, _, client) = setup();
    assert!(client.get_pause_timestamp().is_none());
}

// ── do_action ─────────────────────────────────────────────────────────────────

#[test]
fn do_action_succeeds_when_unpaused() {
    let (env, _, client) = setup();
    let user = Address::generate(&env);
    client.do_action(&user);
}

#[test]
fn do_action_blocked_when_paused() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);

    let user = Address::generate(&env);
    let result = client.try_do_action(&user);
    assert_eq!(result, Err(Ok(Error::ContractPaused)));
}

#[test]
fn do_action_succeeds_after_unpause() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);

    // Pause
    let id1 = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "pause"), &3600);
    client.sign_proposal(&guardian, &id1);
    client.execute_proposal(&admin, &id1);

    // Unpause
    let id2 = client.create_proposal(
        &admin,
        &PauseAction::Unpause,
        &make_str(&env, "resume"),
        &3600,
    );
    client.sign_proposal(&guardian, &id2);
    client.execute_proposal(&admin, &id2);

    let user = Address::generate(&env);
    client.do_action(&user);
}

// ── Full lifecycle cycles ─────────────────────────────────────────────────────

#[test]
fn full_pause_unpause_cycle() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let user = Address::generate(&env);

    assert!(!client.paused());
    client.do_action(&user);

    // Pause
    let id1 = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "emergency"),
        &3600,
    );
    client.sign_proposal(&guardian, &id1);
    client.execute_proposal(&admin, &id1);
    assert!(client.paused());
    assert!(client.try_do_action(&user).is_err());

    // Unpause
    let id2 = client.create_proposal(
        &admin,
        &PauseAction::Unpause,
        &make_str(&env, "resolved"),
        &3600,
    );
    client.sign_proposal(&guardian, &id2);
    client.execute_proposal(&admin, &id2);
    assert!(!client.paused());
    client.do_action(&user);
}

#[test]
fn multiple_pause_unpause_cycles() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let user = Address::generate(&env);

    for _ in 0..3 {
        let id1 =
            client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "pause"), &3600);
        client.sign_proposal(&guardian, &id1);
        client.execute_proposal(&admin, &id1);
        assert!(client.paused());
        assert!(client.try_do_action(&user).is_err());

        let id2 = client.create_proposal(
            &admin,
            &PauseAction::Unpause,
            &make_str(&env, "resume"),
            &3600,
        );
        client.sign_proposal(&guardian, &id2);
        client.execute_proposal(&admin, &id2);
        assert!(!client.paused());
        client.do_action(&user);
    }
}

#[test]
fn proposal_count_increments() {
    let (env, admin, client) = setup();
    assert_eq!(client.proposal_count(), 0);
    client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "p1"), &3600);
    assert_eq!(client.proposal_count(), 1);
    client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "p2"), &3600);
    assert_eq!(client.proposal_count(), 2);
}

// ── Property: only admin can change state ─────────────────────────────────────

#[test]
fn non_admin_cannot_add_guardian() {
    let (env, _, client) = setup();
    let stranger = Address::generate(&env);
    let guardian = Address::generate(&env);
    assert_eq!(
        client.try_add_guardian(&stranger, &guardian),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn non_admin_cannot_remove_guardian() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_remove_guardian(&stranger, &guardian),
        Err(Ok(Error::Unauthorized))
    );
}

#[test]
fn non_admin_cannot_set_threshold() {
    let (env, _, client) = setup();
    let stranger = Address::generate(&env);
    assert_eq!(
        client.try_set_threshold(&stranger, &1),
        Err(Ok(Error::Unauthorized))
    );
}

// ── Additional coverage ───────────────────────────────────────────────────────

#[test]
fn paused_query_is_false_initially() {
    let (_, _, client) = setup();
    assert_eq!(client.paused(), false);
}

#[test]
fn paused_query_is_true_after_pause() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);
    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);
    assert_eq!(client.paused(), true);
}

#[test]
fn paused_query_is_false_after_unpause() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);

    let id1 = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "pause"), &3600);
    client.sign_proposal(&guardian, &id1);
    client.execute_proposal(&admin, &id1);

    let id2 = client.create_proposal(
        &admin,
        &PauseAction::Unpause,
        &make_str(&env, "resume"),
        &3600,
    );
    client.sign_proposal(&guardian, &id2);
    client.execute_proposal(&admin, &id2);

    assert_eq!(client.paused(), false);
}

#[test]
fn admin_can_be_guardian() {
    let (env, admin, client) = setup();
    client.add_guardian(&admin, &admin);
    assert!(client.is_guardian(&admin));
}

#[test]
fn guardian_can_sign_multiple_proposals() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);

    let id1 = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "p1"), &3600);
    let id2 = client.create_proposal(&admin, &PauseAction::Pause, &make_str(&env, "p2"), &3600);

    client.sign_proposal(&guardian, &id1);
    client.sign_proposal(&guardian, &id2);

    assert_eq!(client.get_proposal(&id1).signers.len(), 1);
    assert_eq!(client.get_proposal(&id2).signers.len(), 1);
}

#[test]
fn multiple_guards_can_sign_same_proposal() {
    let (env, admin, client) = setup();
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    client.add_guardian(&admin, &g1);
    client.add_guardian(&admin, &g2);
    client.add_guardian(&admin, &g3);

    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "critical"),
        &3600,
    );

    client.sign_proposal(&g1, &id);
    client.sign_proposal(&g2, &id);
    client.sign_proposal(&g3, &id);

    assert_eq!(client.get_proposal(&id).signers.len(), 3);
}

#[test]
fn threshold_1_allows_single_guardian_execution() {
    let (env, admin, client) = setup_with_threshold(1);
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);

    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "quick pause"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);
    assert!(client.paused());
}

#[test]
fn pause_does_not_change_admin() {
    let (env, admin, client) = setup();
    let guardian = Address::generate(&env);
    client.add_guardian(&admin, &guardian);

    let id = client.create_proposal(
        &admin,
        &PauseAction::Pause,
        &make_str(&env, "reason"),
        &3600,
    );
    client.sign_proposal(&guardian, &id);
    client.execute_proposal(&admin, &id);

    assert_eq!(client.get_admin(), admin);
}
