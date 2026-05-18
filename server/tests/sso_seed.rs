//! Integration tests for `sso::seed::seed_default_from_env`. Covers
//! insert-when-empty, no-op-when-row-exists, and missing-secret-key.

use lets_chat::db::sso_providers;
use lets_chat::sso::{seed, SsoConfig};
use sqlx::SqlitePool;
use url::Url;

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

fn cfg(issuer: &str, autoprovision: bool) -> SsoConfig {
    SsoConfig {
        issuer: Url::parse(issuer).unwrap(),
        client_id: "client-uuid".into(),
        client_secret: "the-secret".into(),
        redirect_uri: Url::parse("https://chat.example/auth/sso/callback").unwrap(),
        autoprovision,
        password_hidden: false,
    }
}

fn key() -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = i as u8;
    }
    k
}

#[tokio::test]
async fn seeds_when_no_row_exists() {
    let pool = setup_pool().await;
    let inserted =
        seed::seed_default_from_env(&pool, Some(&key()), &cfg("https://idp.example/", false))
            .await
            .unwrap();
    assert!(inserted);

    let row = sso_providers::get_provider_by_id(&pool, "default")
        .await
        .unwrap()
        .expect("row inserted");
    assert_eq!(row.kind, "oidc");
    assert_eq!(row.issuer_url, "https://idp.example/");
    assert_eq!(row.client_id, "client-uuid");
    assert!(!row.client_secret_encrypted.is_empty());
    assert!(!row.allow_signup);
    assert!(row.auto_link_verified_email);
    assert!(row.is_enabled(), "seeded row lands live");
}

#[tokio::test]
async fn carries_autoprovision_into_allow_signup() {
    let pool = setup_pool().await;
    seed::seed_default_from_env(&pool, Some(&key()), &cfg("https://idp.example/", true))
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&pool, "default")
        .await
        .unwrap()
        .unwrap();
    assert!(row.allow_signup);
}

#[tokio::test]
async fn no_op_when_row_for_same_issuer_already_exists() {
    let pool = setup_pool().await;
    // First seed creates the row.
    seed::seed_default_from_env(&pool, Some(&key()), &cfg("https://idp.example/", false))
        .await
        .unwrap();
    // Mutate the row out-of-band to prove the seed doesn't overwrite.
    sqlx::query("UPDATE sso_providers SET display_name = ?, client_id = ? WHERE id = 'default'")
        .bind("Edited by admin")
        .bind("edited-client")
        .execute(&pool)
        .await
        .unwrap();
    // Second seed call with the same issuer is a no-op.
    let inserted =
        seed::seed_default_from_env(&pool, Some(&key()), &cfg("https://idp.example/", false))
            .await
            .unwrap();
    assert!(!inserted);

    let row = sso_providers::get_provider_by_id(&pool, "default")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.display_name, "Edited by admin");
    assert_eq!(row.client_id, "edited-client");
}

#[tokio::test]
async fn errors_when_secret_key_missing() {
    let pool = setup_pool().await;
    let err = seed::seed_default_from_env(&pool, None, &cfg("https://idp.example/", false))
        .await
        .unwrap_err();
    assert!(matches!(err, seed::SeedError::NoSecretKey));
    // Row never inserted.
    assert!(sso_providers::get_provider_by_id(&pool, "default")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn encrypted_secret_round_trips() {
    let pool = setup_pool().await;
    seed::seed_default_from_env(&pool, Some(&key()), &cfg("https://idp.example/", false))
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&pool, "default")
        .await
        .unwrap()
        .unwrap();
    let plain = lets_chat::sso::secret::decrypt_client_secret(&key(), &row.client_secret_encrypted)
        .unwrap();
    assert_eq!(plain, "the-secret");
}
