//! Integration tests for `db::sso`. Each test opens its own fresh
//! in-memory SQLite, applies the auth migration set, then exercises
//! one helper end-to-end. See docs/lets-chat/sso/05-schema-and-account-linking.md.

use lets_chat::db;
use lets_chat::db::sso::{CreateUserFromSso, SsoFlow};
use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
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
        include_str!("../migrations/auth/0018_sso_flows_provider.sql"),
        include_str!("../migrations/auth/0019_sso_group_mappings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

const ISSUER: &str = "https://msp-api.example.com";

// ---------------------------------------------------------------------------
// link / find / unlink
// ---------------------------------------------------------------------------

#[tokio::test]
async fn link_creates_row_and_find_resolves() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    db::sso::link_sso_identity(
        &pool,
        &alice,
        ISSUER,
        "alice-sub-123",
        Some("alice@example.com"),
        false,
    )
    .await
    .unwrap();

    let resolved = db::sso::find_user_by_sso(&pool, ISSUER, "alice-sub-123")
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(alice.as_str()));
}

#[tokio::test]
async fn find_by_sso_returns_none_for_unknown() {
    let pool = setup_pool().await;
    let resolved = db::sso::find_user_by_sso(&pool, ISSUER, "never-existed")
        .await
        .unwrap();
    assert!(resolved.is_none());
}

#[tokio::test]
async fn find_by_sso_excludes_banned_users() {
    let pool = setup_pool().await;
    let bob = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    db::sso::link_sso_identity(&pool, &bob, ISSUER, "bob-sub", None, false)
        .await
        .unwrap();
    // Banned -> excluded.
    sqlx::query("UPDATE users SET is_banned = 1 WHERE id = ?")
        .bind(&bob)
        .execute(&pool)
        .await
        .unwrap();
    let resolved = db::sso::find_user_by_sso(&pool, ISSUER, "bob-sub")
        .await
        .unwrap();
    assert!(resolved.is_none());
}

#[tokio::test]
async fn link_is_idempotent_and_updates_last_seen() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    db::sso::link_sso_identity(&pool, &alice, ISSUER, "sub-1", Some("a@x"), false)
        .await
        .unwrap();
    // Second call with the SAME (issuer, subject) refreshes the row.
    db::sso::link_sso_identity(&pool, &alice, ISSUER, "sub-1", Some("a2@x"), false)
        .await
        .unwrap();

    let rows = db::sso::list_sso_identities_for_user(&pool, &alice)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "second link upserted, did not duplicate");
    // The email reflects the upserted (later) value.
    assert_eq!(rows[0].email.as_deref(), Some("a2@x"));
}

#[tokio::test]
async fn link_preserves_email_when_upsert_passes_none() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&pool, &alice, ISSUER, "sub-1", Some("first@x"), false)
        .await
        .unwrap();
    // Subsequent upsert with email=None must NOT clear the stored email
    // (COALESCE on the ON CONFLICT clause).
    db::sso::link_sso_identity(&pool, &alice, ISSUER, "sub-1", None, false)
        .await
        .unwrap();

    let rows = db::sso::list_sso_identities_for_user(&pool, &alice)
        .await
        .unwrap();
    assert_eq!(rows[0].email.as_deref(), Some("first@x"));
}

#[tokio::test]
async fn unlink_removes_only_target_users_row() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let bob = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    db::sso::link_sso_identity(&pool, &alice, ISSUER, "alice-sub", None, false)
        .await
        .unwrap();
    db::sso::link_sso_identity(&pool, &bob, ISSUER, "bob-sub", None, false)
        .await
        .unwrap();

    let removed = db::sso::unlink_sso_identity(&pool, &alice).await.unwrap();
    assert_eq!(removed, 1);

    // Alice unlinked, Bob still there.
    assert!(db::sso::find_user_by_sso(&pool, ISSUER, "alice-sub")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db::sso::find_user_by_sso(&pool, ISSUER, "bob-sub")
            .await
            .unwrap()
            .as_deref(),
        Some(bob.as_str())
    );
}

#[tokio::test]
async fn unlink_is_noop_on_user_with_no_identity() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let removed = db::sso::unlink_sso_identity(&pool, &alice).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn cascade_delete_removes_link_when_user_dropped() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::sso::link_sso_identity(&pool, &alice, ISSUER, "sub-1", None, false)
        .await
        .unwrap();
    // Simulate full account delete via the existing helper.
    db::auth::delete_user(&pool, &alice).await.unwrap();
    assert!(db::sso::find_user_by_sso(&pool, ISSUER, "sub-1")
        .await
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------------------
// user_has_password
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_has_password_true_when_hash_present() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "the-hash")
        .await
        .unwrap();
    assert!(db::sso::user_has_password(&pool, &alice).await.unwrap());
}

#[tokio::test]
async fn user_has_password_false_when_hash_null() {
    let pool = setup_pool().await;
    // SSO-only user via create_user_from_sso (password_hash = NULL).
    let uid = db::sso::create_user_from_sso(
        &pool,
        CreateUserFromSso {
            issuer: ISSUER,
            subject: "sso-sub-1",
            email: Some("sso@example.com"),
            preferred_username: Some("ssouser"),
            display_name: Some("SSO User"),
        },
    )
    .await
    .unwrap();
    assert!(!db::sso::user_has_password(&pool, &uid).await.unwrap());
}

#[tokio::test]
async fn user_has_password_false_for_unknown_user() {
    let pool = setup_pool().await;
    assert!(!db::sso::user_has_password(&pool, "never-existed")
        .await
        .unwrap());
}

// ---------------------------------------------------------------------------
// create_user_from_sso
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_user_from_sso_inserts_both_rows() {
    let pool = setup_pool().await;
    let uid = db::sso::create_user_from_sso(
        &pool,
        CreateUserFromSso {
            issuer: ISSUER,
            subject: "fresh-sub",
            email: Some("fresh@example.com"),
            preferred_username: Some("fresh"),
            display_name: Some("Fresh User"),
        },
    )
    .await
    .unwrap();

    // users row landed.
    let user = db::auth::find_user_by_id(&pool, &uid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "fresh");
    assert_eq!(user.display_name.as_deref(), Some("Fresh User"));
    // sso_identities row landed. auto_linked stays false: the
    // autoprovision path is "no existing user matched, create one",
    // not "auto-linked on email collision" (doc 02 section 2).
    let rows = db::sso::list_sso_identities_for_user(&pool, &uid)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject, "fresh-sub");
    assert!(!rows[0].auto_linked);
}

#[tokio::test]
async fn create_user_from_sso_resolves_username_collision() {
    let pool = setup_pool().await;
    // Pre-existing local user takes "alice".
    db::auth::create_user(&pool, "alice", "hash").await.unwrap();

    let uid = db::sso::create_user_from_sso(
        &pool,
        CreateUserFromSso {
            issuer: ISSUER,
            subject: "alice-via-sso",
            email: Some("alice@example.com"),
            preferred_username: Some("alice"),
            display_name: None,
        },
    )
    .await
    .unwrap();
    let user = db::auth::find_user_by_id(&pool, &uid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "alice-2");
}

#[tokio::test]
async fn create_user_from_sso_falls_back_when_username_unusable() {
    let pool = setup_pool().await;
    let uid = db::sso::create_user_from_sso(
        &pool,
        CreateUserFromSso {
            issuer: ISSUER,
            // Subject's first 8 ascii-alphanumeric chars: "abcd1234".
            subject: "abcd1234-rest-of-subject",
            email: None,
            preferred_username: Some("@@@@"), // Sanitizes to "".
            display_name: None,
        },
    )
    .await
    .unwrap();
    let user = db::auth::find_user_by_id(&pool, &uid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "sso-abcd1234");
}

// ---------------------------------------------------------------------------
// sso_flows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_then_consume_flow_returns_row_then_none() {
    let pool = setup_pool().await;
    db::sso::insert_sso_flow(
        &pool,
        "flow-abc",
        "csrf-x",
        "nonce-y",
        "verifier-z",
        "/rooms/general",
        "sign_in",
        None,
        "default",
        600,
    )
    .await
    .unwrap();

    let consumed: Option<SsoFlow> = db::sso::consume_sso_flow(&pool, "flow-abc").await.unwrap();
    let row = consumed.expect("first consume returns the row");
    assert_eq!(row.csrf_state, "csrf-x");
    assert_eq!(row.nonce, "nonce-y");
    assert_eq!(row.pkce_verifier, "verifier-z");
    assert_eq!(row.return_to, "/rooms/general");
    assert_eq!(row.kind, "sign_in");
    assert_eq!(row.provider_id, "default");
    assert!(row.user_id.is_none());

    // Second consume returns None - one-shot semantics.
    let again = db::sso::consume_sso_flow(&pool, "flow-abc").await.unwrap();
    assert!(again.is_none());
}

#[tokio::test]
async fn consume_returns_none_for_unknown_flow_id() {
    let pool = setup_pool().await;
    assert!(db::sso::consume_sso_flow(&pool, "never-existed")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn link_flow_carries_user_id() {
    let pool = setup_pool().await;
    let alice = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    db::sso::insert_sso_flow(
        &pool,
        "link-flow",
        "csrf",
        "nonce",
        "verifier",
        "/settings/profile",
        "link",
        Some(&alice),
        "default",
        600,
    )
    .await
    .unwrap();
    let row = db::sso::consume_sso_flow(&pool, "link-flow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "link");
    assert_eq!(row.user_id.as_deref(), Some(alice.as_str()));
}

#[tokio::test]
async fn expired_flow_is_not_consumable() {
    let pool = setup_pool().await;
    // ttl = -10 seconds: the row is inserted ALREADY expired.
    db::sso::insert_sso_flow(
        &pool,
        "expired-flow",
        "c",
        "n",
        "v",
        "/",
        "sign_in",
        None,
        "default",
        -10,
    )
    .await
    .unwrap();
    let row = db::sso::consume_sso_flow(&pool, "expired-flow")
        .await
        .unwrap();
    assert!(row.is_none(), "expired row must not be returned");
}

#[tokio::test]
async fn prune_drops_only_expired_rows() {
    let pool = setup_pool().await;
    db::sso::insert_sso_flow(
        &pool,
        "fresh-flow",
        "c",
        "n",
        "v",
        "/",
        "sign_in",
        None,
        "default",
        600,
    )
    .await
    .unwrap();
    db::sso::insert_sso_flow(
        &pool,
        "stale-flow",
        "c",
        "n",
        "v",
        "/",
        "sign_in",
        None,
        "default",
        -1,
    )
    .await
    .unwrap();

    let pruned = db::sso::prune_expired_sso_flows(&pool).await.unwrap();
    assert_eq!(pruned, 1, "exactly one stale row removed");

    // The fresh flow is still consumable.
    assert!(db::sso::consume_sso_flow(&pool, "fresh-flow")
        .await
        .unwrap()
        .is_some());
}
