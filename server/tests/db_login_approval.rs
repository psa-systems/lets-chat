//! LC-587: suspicious-login approval + known-device DB/service invariants.
//!
//! Covers the flagged -> approved recovery, single-use, attempt-cap, and
//! device-baseline behaviour at the DB + service layer. The pure decision logic
//! (new-device rule, suspicious combine, code shape) is unit-tested inside
//! `login_approval`. A full HTTP callback test would need a mock Bunyip OP;
//! `verify` / `apply_baseline` take the auth pool directly so the recovery path
//! is exercised here without standing up the whole AppState.

mod common;

use lets_chat::db;
use lets_chat::login_approval::{self, VerifyOutcome};

/// SHA-256 hex of the code, matching what the service stores (the session-token
/// hasher is the same SHA-256 hex).
fn code_hash(code: &str) -> String {
    db::auth::hash_session_token(code)
}

#[tokio::test]
async fn flagged_login_then_approved_recovery() {
    let auth = common::auth_pool().await;
    let uid = db::auth::create_user(&auth, "alice", "h").await.unwrap();

    // A flagged login (new country + new device) inserts a pending challenge.
    db::auth::insert_login_approval(
        &auth,
        "tok-1",
        &uid,
        &code_hash("123456"),
        Some("US"),
        Some("dev-hash-a"),
        Some("8.8.8.8"),
        Some("Firefox"),
    )
    .await
    .unwrap();

    // A wrong code does not complete the login.
    assert!(matches!(
        login_approval::verify(&auth, "tok-1", "000000").await,
        VerifyOutcome::Wrong
    ));

    // The correct code approves and returns the flagged context to seed the
    // baseline.
    let VerifyOutcome::Approved {
        user_id,
        country,
        device_hash,
    } = login_approval::verify(&auth, "tok-1", "123456").await
    else {
        panic!("expected Approved");
    };
    assert_eq!(user_id, uid);
    assert_eq!(country.as_deref(), Some("US"));
    assert_eq!(device_hash.as_deref(), Some("dev-hash-a"));

    // Single-use: the same code cannot be replayed.
    assert!(matches!(
        login_approval::verify(&auth, "tok-1", "123456").await,
        VerifyOutcome::Invalid
    ));

    // Applying the approved context makes the same country + device unremarkable
    // on the next login.
    login_approval::apply_baseline(
        &auth,
        &uid,
        country.as_deref(),
        device_hash.as_deref(),
        Some("Firefox"),
    )
    .await;
    assert_eq!(
        db::auth::get_last_login_country(&auth, &uid)
            .await
            .unwrap()
            .as_deref(),
        Some("US")
    );
    assert!(db::auth::is_known_device(&auth, &uid, "dev-hash-a")
        .await
        .unwrap());
}

#[tokio::test]
async fn attempt_cap_burns_the_challenge() {
    let auth = common::auth_pool().await;
    let uid = db::auth::create_user(&auth, "bob", "h").await.unwrap();
    db::auth::insert_login_approval(
        &auth,
        "tok-2",
        &uid,
        &code_hash("654321"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Wrong codes up to the cap keep returning Wrong...
    for _ in 0..(login_approval::MAX_ATTEMPTS - 1) {
        assert!(matches!(
            login_approval::verify(&auth, "tok-2", "111111").await,
            VerifyOutcome::Wrong
        ));
    }
    // ...the attempt that hits the cap burns the challenge.
    assert!(matches!(
        login_approval::verify(&auth, "tok-2", "111111").await,
        VerifyOutcome::Invalid
    ));
    // Even the correct code no longer works once burned.
    assert!(matches!(
        login_approval::verify(&auth, "tok-2", "654321").await,
        VerifyOutcome::Invalid
    ));
}

#[tokio::test]
async fn unknown_or_expired_token_is_invalid() {
    let auth = common::auth_pool().await;
    // Never-issued token.
    assert!(matches!(
        login_approval::verify(&auth, "nope", "123456").await,
        VerifyOutcome::Invalid
    ));
}

#[tokio::test]
async fn device_baseline_then_known() {
    let auth = common::auth_pool().await;
    let uid = db::auth::create_user(&auth, "carol", "h").await.unwrap();

    // No devices yet: baseline (the first device must never be flagged).
    assert!(!db::auth::has_known_device(&auth, &uid).await.unwrap());

    db::auth::record_known_device(&auth, &uid, "dev-1", Some("UA"))
        .await
        .unwrap();
    assert!(db::auth::has_known_device(&auth, &uid).await.unwrap());
    assert!(db::auth::is_known_device(&auth, &uid, "dev-1")
        .await
        .unwrap());
    assert!(!db::auth::is_known_device(&auth, &uid, "dev-2")
        .await
        .unwrap());

    // Recording is idempotent on (user, device_hash).
    db::auth::record_known_device(&auth, &uid, "dev-1", Some("UA2"))
        .await
        .unwrap();
    assert!(db::auth::is_known_device(&auth, &uid, "dev-1")
        .await
        .unwrap());
}
