//! DEV-300: the dev-only `SETUP_DEFAULT_ADMIN` bootstrap.

use sqlx::SqlitePool;

mod common;

const CONFIGURED: &str = "admin@a8n.run:admin1234";

async fn user_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("count users")
}

async fn admin_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
        .fetch_one(pool)
        .await
        .expect("count admins")
}

#[tokio::test]
async fn dev_mode_seeds_one_admin_the_sso_callback_can_adopt() {
    let pool = common::auth_pool().await;
    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;

    assert_eq!(admin_count(&pool).await, 1, "exactly one admin");
    let row: (String, String, String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT username, role, bunyip_sub, email, email_verified_at FROM users")
            .fetch_one(&pool)
            .await
            .expect("the seeded row");
    assert_eq!(row.0, "admin", "handle derives from the email local part");
    assert_eq!(row.1, "admin");
    assert_eq!(row.2, "", "unlinked, so the first sign-in adopts it");
    assert_eq!(row.3.as_deref(), Some("admin@a8n.run"));
    assert!(
        row.4.is_some(),
        "email must be verified or resolve_or_provision_user refuses to adopt it"
    );
}

#[tokio::test]
async fn a_second_boot_neither_duplicates_nor_errors() {
    let pool = common::auth_pool().await;
    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;
    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;
    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;

    assert_eq!(
        user_count(&pool).await,
        1,
        "still one row after three boots"
    );
    assert_eq!(admin_count(&pool).await, 1);
}

#[tokio::test]
async fn a_release_build_writes_nothing_even_when_configured() {
    // The production gate. Same value, same empty database; only the build
    // profile differs, and nothing may be created.
    let pool = common::auth_pool().await;
    lets_chat::setup_admin::seed_default_admin(&pool, false, Some(CONFIGURED)).await;

    assert_eq!(
        user_count(&pool).await,
        0,
        "no user seeded on a release build"
    );
    assert_eq!(admin_count(&pool).await, 0);
}

#[tokio::test]
async fn an_unconfigured_dev_build_writes_nothing() {
    let pool = common::auth_pool().await;
    for raw in [None, Some(""), Some("   ")] {
        lets_chat::setup_admin::seed_default_admin(&pool, true, raw).await;
    }
    assert_eq!(user_count(&pool).await, 0);
}

#[tokio::test]
async fn a_malformed_value_writes_nothing() {
    let pool = common::auth_pool().await;
    for raw in ["admin@a8n.run", ":admin1234", "admin:admin1234"] {
        lets_chat::setup_admin::seed_default_admin(&pool, true, Some(raw)).await;
    }
    assert_eq!(user_count(&pool).await, 0);
}

#[tokio::test]
async fn an_existing_admin_is_left_alone() {
    let pool = common::auth_pool().await;
    let existing = lets_chat::db::auth::create_user(&pool, "boss", "")
        .await
        .expect("create user");
    lets_chat::db::auth::set_user_role(&pool, &existing, "admin")
        .await
        .expect("promote");

    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;

    assert_eq!(user_count(&pool).await, 1, "no second admin seeded");
    assert_eq!(admin_count(&pool).await, 1);
}

#[tokio::test]
async fn a_taken_handle_is_suffixed_rather_than_colliding() {
    // A dev database with users but no admin is exactly the state the seed is
    // for, and its handle may already be taken.
    let pool = common::auth_pool().await;
    lets_chat::db::auth::create_user(&pool, "admin", "")
        .await
        .expect("create user");

    lets_chat::setup_admin::seed_default_admin(&pool, true, Some(CONFIGURED)).await;

    assert_eq!(admin_count(&pool).await, 1);
    let handle: String = sqlx::query_scalar("SELECT username FROM users WHERE role = 'admin'")
        .fetch_one(&pool)
        .await
        .expect("the seeded row");
    assert_eq!(handle, "admin-2");
}
