//! Phase 22 task 5: admin-configurable default for the "new user starts
//! with email digest enabled" preference. The toggle lives in the
//! generic `settings` key-value table under `default_notify_email_digest`
//! and is applied to a new user only at registration time.
//!
//! This test pins three properties:
//!   1. Absent key => new users start with the schema default (0).
//!   2. Setting flipped to "1" => helper applies it to a new user.
//!   3. Toggling the flag back to "0" does NOT touch users already
//!      created under the previous value (existing users keep their
//!      preference; only future registrations are affected).

use sqlx::SqlitePool;

async fn setup_auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
        include_str!("../migrations/auth/0008_two_factor.sql"),
        include_str!("../migrations/auth/0009_push_subscriptions.sql"),
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
        include_str!("../migrations/auth/0012_session_metadata.sql"),
        include_str!("../migrations/auth/0013_digest_columns.sql"),
        include_str!("../migrations/auth/0014_login_alerts.sql"),
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sso_identities.sql"),
        include_str!("../migrations/auth/0017_sso_providers.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn setup_settings_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/settings/0001_create_tables.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn default_off_when_setting_absent() {
    let auth = setup_auth_pool().await;
    let id = lets_chat::db::auth::create_user(&auth, "alice", "h")
        .await
        .unwrap();
    let rec = lets_chat::db::auth::find_user_by_id(&auth, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !rec.notify_email_digest_enabled,
        "schema default for the column is 0; admin has not opted users in"
    );
}

#[tokio::test]
async fn flag_propagates_to_new_users_via_setter() {
    // Models what the register handler does: check the setting after
    // create_user, apply if "1".
    let auth = setup_auth_pool().await;
    let settings = setup_settings_pool().await;
    lets_chat::db::settings::set_setting(&settings, "default_notify_email_digest", "1")
        .await
        .unwrap();

    let id = lets_chat::db::auth::create_user(&auth, "alice", "h")
        .await
        .unwrap();
    if lets_chat::db::settings::get_setting(&settings, "default_notify_email_digest")
        .await
        .unwrap()
        .as_deref()
        == Some("1")
    {
        lets_chat::db::auth::set_notify_email_digest_enabled(&auth, &id, true)
            .await
            .unwrap();
    }

    let rec = lets_chat::db::auth::find_user_by_id(&auth, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.notify_email_digest_enabled);
}

#[tokio::test]
async fn toggling_default_off_does_not_change_existing_users() {
    // Register-time application is one-shot. If the operator later
    // flips the flag off, users who registered under the "on" regime
    // keep their preference until they manually toggle it.
    let auth = setup_auth_pool().await;
    let settings = setup_settings_pool().await;
    lets_chat::db::settings::set_setting(&settings, "default_notify_email_digest", "1")
        .await
        .unwrap();
    let alice = lets_chat::db::auth::create_user(&auth, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_notify_email_digest_enabled(&auth, &alice, true)
        .await
        .unwrap();

    // Flip the instance default off. Existing users are not touched.
    lets_chat::db::settings::set_setting(&settings, "default_notify_email_digest", "0")
        .await
        .unwrap();

    let rec = lets_chat::db::auth::find_user_by_id(&auth, &alice)
        .await
        .unwrap()
        .unwrap();
    assert!(
        rec.notify_email_digest_enabled,
        "existing users keep their previous opt-in state"
    );

    // A user registered AFTER the flip does NOT auto-opt-in.
    let bob = lets_chat::db::auth::create_user(&auth, "bob", "h")
        .await
        .unwrap();
    if lets_chat::db::settings::get_setting(&settings, "default_notify_email_digest")
        .await
        .unwrap()
        .as_deref()
        == Some("1")
    {
        lets_chat::db::auth::set_notify_email_digest_enabled(&auth, &bob, true)
            .await
            .unwrap();
    }
    let bob_rec = lets_chat::db::auth::find_user_by_id(&auth, &bob)
        .await
        .unwrap()
        .unwrap();
    assert!(!bob_rec.notify_email_digest_enabled);
}

#[tokio::test]
async fn set_email_round_trip_with_null_for_empty() {
    let auth = setup_auth_pool().await;
    let id = lets_chat::db::auth::create_user(&auth, "alice", "h")
        .await
        .unwrap();

    // Initial state: column is NULL.
    let rec = lets_chat::db::auth::find_user_by_id(&auth, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.email.is_none());

    lets_chat::db::auth::set_user_email(&auth, &id, Some("alice@example.com"))
        .await
        .unwrap();
    let rec = lets_chat::db::auth::find_user_by_id(&auth, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.email.as_deref(), Some("alice@example.com"));

    // Clearing the field stores NULL, not the empty string. The
    // eligibility query filters both equivalently, but downstream code
    // can rely on "NULL means unset" without also checking for "".
    lets_chat::db::auth::set_user_email(&auth, &id, None)
        .await
        .unwrap();
    let rec = lets_chat::db::auth::find_user_by_id(&auth, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.email.is_none());
}
