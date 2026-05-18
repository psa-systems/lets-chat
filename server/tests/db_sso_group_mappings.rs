//! Integration tests for `db::sso_group_mappings`. Open an in-memory
//! pool with the full auth migration set, exercise the CRUD helpers.
//! Group-claim -> enclave sync lives in L17; this phase just nails
//! down the table + helpers.

use lets_chat::db::sso_group_mappings as gm;
use lets_chat::db::sso_providers::{self, InsertProvider};
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
        include_str!("../migrations/auth/0020_session_tenant.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn seed_provider(pool: &SqlitePool, id: &str) {
    sso_providers::insert_provider(
        pool,
        InsertProvider {
            id,
            kind: "oidc",
            display_name: "Stub",
            issuer_url: &format!("https://idp-{id}/"),
            client_id: "c",
            client_secret_encrypted: b"s",
            scopes: "openid",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn insert_then_list_for_provider() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    let id = gm::insert(&pool, "acme", "engineering", 42, "Moderator")
        .await
        .unwrap();
    assert!(id > 0);

    let rows = gm::list_for_provider(&pool, "acme").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group_value, "engineering");
    assert_eq!(rows[0].enclave_id, 42);
    assert_eq!(rows[0].role, "Moderator");
}

#[tokio::test]
async fn unique_triple_rejected() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    let err = gm::insert(&pool, "acme", "eng", 1, "Admin")
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("unique"));
}

#[tokio::test]
async fn same_group_can_map_to_multiple_enclaves() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "eng", 2, "User").await.unwrap();
    let rows = gm::list_for_group(&pool, "acme", "eng").await.unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn list_for_provider_orders_by_group_value() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    gm::insert(&pool, "acme", "zulu", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "alpha", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "mike", 1, "User").await.unwrap();
    let groups: Vec<String> = gm::list_for_provider(&pool, "acme")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.group_value)
        .collect();
    assert_eq!(groups, vec!["alpha", "mike", "zulu"]);
}

#[tokio::test]
async fn list_for_group_returns_only_matching_rows() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    seed_provider(&pool, "other").await;
    gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "ops", 2, "User").await.unwrap();
    gm::insert(&pool, "other", "eng", 3, "User").await.unwrap();

    let rows = gm::list_for_group(&pool, "acme", "eng").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].enclave_id, 1);
}

#[tokio::test]
async fn update_role_changes_only_role() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    let id = gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    let n = gm::update_role(&pool, id, "Admin").await.unwrap();
    assert_eq!(n, 1);
    let rows = gm::list_for_provider(&pool, "acme").await.unwrap();
    assert_eq!(rows[0].role, "Admin");
    assert_eq!(rows[0].enclave_id, 1);
    assert_eq!(rows[0].group_value, "eng");
}

#[tokio::test]
async fn update_role_missing_id_is_noop() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    let n = gm::update_role(&pool, 9999, "Admin").await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn delete_removes_one_row() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    let id = gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "ops", 2, "User").await.unwrap();
    let n = gm::delete(&pool, id).await.unwrap();
    assert_eq!(n, 1);
    let rows = gm::list_for_provider(&pool, "acme").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].group_value, "ops");
}

#[tokio::test]
async fn delete_all_for_provider_clears_only_that_provider() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    seed_provider(&pool, "other").await;
    gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "ops", 2, "User").await.unwrap();
    gm::insert(&pool, "other", "eng", 3, "User").await.unwrap();

    let n = gm::delete_all_for_provider(&pool, "acme").await.unwrap();
    assert_eq!(n, 2);
    assert!(gm::list_for_provider(&pool, "acme")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        gm::list_for_provider(&pool, "other").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn cascade_delete_removes_mappings_when_provider_deleted() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    gm::insert(&pool, "acme", "eng", 1, "User").await.unwrap();
    gm::insert(&pool, "acme", "ops", 2, "User").await.unwrap();
    // Enable FK enforcement (off by default in SQLite for ATTACH-based
    // pools; sqlx ::memory: respects it but requires the pragma).
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    sso_providers::delete_provider(&pool, "acme").await.unwrap();
    let rows = gm::list_for_provider(&pool, "acme").await.unwrap();
    assert!(rows.is_empty(), "cascade removed mappings on provider drop");
}

#[tokio::test]
async fn role_check_constraint_rejects_unknown_role() {
    let pool = setup_pool().await;
    seed_provider(&pool, "acme").await;
    let err = gm::insert(&pool, "acme", "eng", 1, "SuperAdmin")
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected CHECK violation, got: {err}"
    );
}
