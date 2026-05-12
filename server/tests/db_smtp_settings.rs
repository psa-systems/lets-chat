//! Tests for `db::smtp_settings` (phase 22 task 2).
//!
//! The migration discards the previous plaintext SMTP password during
//! upgrade. From task 2 onwards the password is AES-256-GCM-encrypted at
//! rest under a key derived from `LETS_CHAT_SECRET_KEY`. These tests
//! verify the round-trip and the "blank password = leave existing alone"
//! behaviour that the admin form relies on.

use lets_chat::db::smtp_settings::{self, SmtpConfigInput, TlsMode};
use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/settings/0001_create_tables.sql"),
        include_str!("../migrations/settings/0002_uploads.sql"),
        include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        include_str!("../migrations/settings/0004_smtp_settings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn test_key() -> [u8; 32] {
    [42u8; 32]
}

#[tokio::test]
async fn migration_inserts_default_row() {
    let pool = setup_pool().await;
    let cfg = smtp_settings::load(&pool, &test_key()).await.unwrap();
    let cfg = cfg.expect("default row should be inserted by migration");
    assert_eq!(cfg.host, "");
    assert_eq!(cfg.port, 587);
    assert!(cfg.username.is_none());
    assert!(cfg.password.is_none());
    assert_eq!(cfg.from_address, "");
    assert_eq!(cfg.tls_mode, TlsMode::StartTls);
}

#[tokio::test]
async fn save_and_load_round_trips_password_through_aes_gcm() {
    let pool = setup_pool().await;
    let key = test_key();
    let input = SmtpConfigInput {
        host: "smtp.example.com".into(),
        port: 587,
        username: Some("relay@example.com".into()),
        password: Some("hunter2".into()),
        from_address: "noreply@example.com".into(),
        tls_mode: TlsMode::StartTls,
    };
    smtp_settings::save(&pool, &key, &input).await.unwrap();
    let cfg = smtp_settings::load(&pool, &key).await.unwrap().unwrap();
    assert_eq!(cfg.host, "smtp.example.com");
    assert_eq!(cfg.port, 587);
    assert_eq!(cfg.username.as_deref(), Some("relay@example.com"));
    assert_eq!(cfg.password.as_deref(), Some("hunter2"));
    assert_eq!(cfg.from_address, "noreply@example.com");
    assert_eq!(cfg.tls_mode, TlsMode::StartTls);
}

#[tokio::test]
async fn save_with_no_password_leaves_existing_password_alone() {
    let pool = setup_pool().await;
    let key = test_key();
    smtp_settings::save(
        &pool,
        &key,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: Some("alice".into()),
            password: Some("first-password".into()),
            from_address: "noreply@example.com".into(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();

    // Subsequent save without a password (form-blank case) must NOT
    // clobber the encrypted password. Other fields update normally.
    smtp_settings::save(
        &pool,
        &key,
        &SmtpConfigInput {
            host: "smtp2.example.com".into(),
            port: 465,
            username: Some("bob".into()),
            password: None,
            from_address: "other@example.com".into(),
            tls_mode: TlsMode::Tls,
        },
    )
    .await
    .unwrap();

    let cfg = smtp_settings::load(&pool, &key).await.unwrap().unwrap();
    assert_eq!(cfg.host, "smtp2.example.com");
    assert_eq!(cfg.port, 465);
    assert_eq!(cfg.username.as_deref(), Some("bob"));
    assert_eq!(cfg.from_address, "other@example.com");
    assert_eq!(cfg.tls_mode, TlsMode::Tls);
    assert_eq!(
        cfg.password.as_deref(),
        Some("first-password"),
        "password should survive a save-without-password"
    );
}

#[tokio::test]
async fn save_with_empty_password_string_also_leaves_existing_alone() {
    // A blank `<input type=password>` is delivered to the route as
    // `smtp_pass=""`. The admin route maps that to `password: None`,
    // but db::smtp_settings::save also treats `Some("")` as "leave
    // alone" defensively; assert that contract.
    let pool = setup_pool().await;
    let key = test_key();
    smtp_settings::save(
        &pool,
        &key,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: None,
            password: Some("first".into()),
            from_address: "x@example.com".into(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();
    smtp_settings::save(
        &pool,
        &key,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: None,
            password: Some(String::new()),
            from_address: "y@example.com".into(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();
    let cfg = smtp_settings::load(&pool, &key).await.unwrap().unwrap();
    assert_eq!(cfg.password.as_deref(), Some("first"));
    assert_eq!(cfg.from_address, "y@example.com");
}

#[tokio::test]
async fn load_with_wrong_key_returns_err() {
    let pool = setup_pool().await;
    let saved_key = test_key();
    smtp_settings::save(
        &pool,
        &saved_key,
        &SmtpConfigInput {
            host: "smtp.example.com".into(),
            port: 587,
            username: None,
            password: Some("secret".into()),
            from_address: "x@example.com".into(),
            tls_mode: TlsMode::StartTls,
        },
    )
    .await
    .unwrap();

    let wrong_key = [99u8; 32];
    let err = smtp_settings::load(&pool, &wrong_key).await;
    assert!(
        err.is_err(),
        "decrypting with the wrong key must fail loudly"
    );
}

#[tokio::test]
async fn tls_mode_round_trips_for_each_variant() {
    let pool = setup_pool().await;
    let key = test_key();
    for mode in [TlsMode::StartTls, TlsMode::Tls, TlsMode::None] {
        smtp_settings::save(
            &pool,
            &key,
            &SmtpConfigInput {
                host: "smtp.example.com".into(),
                port: 587,
                username: None,
                password: None,
                from_address: "x@example.com".into(),
                tls_mode: mode,
            },
        )
        .await
        .unwrap();
        let cfg = smtp_settings::load(&pool, &key).await.unwrap().unwrap();
        assert_eq!(cfg.tls_mode, mode);
    }
}
