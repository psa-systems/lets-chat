use lets_chat::db;
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
        include_str!("../migrations/auth/0016_sidebar_categories.sql"),
        include_str!("../migrations/auth/0017_drop_sidebar_categories_add_collapsed.sql"),
        include_str!("../migrations/auth/0018_starred_rooms.sql"),
        include_str!("../migrations/auth/0019_api_tokens.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn new_user_starts_with_2fa_disabled() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "alice", "hash").await.unwrap();
    let u = db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!u.totp_enabled);
    assert!(u.totp_secret_encrypted.is_none());
    assert!(u.totp_nonce.is_none());
    assert!(u.totp_recovery_hashes.is_none());
}

#[tokio::test]
async fn set_secret_then_enable_round_trip() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "bob", "hash").await.unwrap();
    let secret = b"a-fake-secret".to_vec();
    let nonce = b"twelve-bytes".to_vec();
    db::two_factor::set_totp_secret(&pool, &id, &secret, &nonce)
        .await
        .unwrap();
    let u = db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(u.totp_secret_encrypted.as_deref(), Some(secret.as_slice()));
    assert_eq!(u.totp_nonce.as_deref(), Some(nonce.as_slice()));
    assert!(!u.totp_enabled, "must remain disabled until confirmed");

    db::two_factor::enable_totp(&pool, &id, "[\"hash1\",\"hash2\"]")
        .await
        .unwrap();
    let u = db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(u.totp_enabled);
    assert_eq!(
        u.totp_recovery_hashes.as_deref(),
        Some("[\"hash1\",\"hash2\"]")
    );
}

#[tokio::test]
async fn pending_2fa_token_round_trip() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "carol", "hash").await.unwrap();
    let token = db::two_factor::create_pending_2fa(&pool, &id)
        .await
        .unwrap();
    assert_eq!(token.len(), 64);

    let resolved = db::two_factor::get_pending_2fa_user(&pool, &token)
        .await
        .unwrap();
    assert_eq!(resolved.as_deref(), Some(id.as_str()));

    db::two_factor::delete_pending_2fa(&pool, &token)
        .await
        .unwrap();
    let after = db::two_factor::get_pending_2fa_user(&pool, &token)
        .await
        .unwrap();
    assert!(after.is_none(), "token must be single-use");
}

#[tokio::test]
async fn pending_2fa_unknown_token_returns_none() {
    let pool = setup_pool().await;
    let r = db::two_factor::get_pending_2fa_user(&pool, "does-not-exist")
        .await
        .unwrap();
    assert!(r.is_none());
}

#[tokio::test]
async fn disable_clears_all_totp_fields() {
    let pool = setup_pool().await;
    let id = db::auth::create_user(&pool, "dave", "hash").await.unwrap();
    db::two_factor::set_totp_secret(&pool, &id, b"sec", b"twelve-bytes!")
        .await
        .unwrap();
    db::two_factor::enable_totp(&pool, &id, "[\"x\"]")
        .await
        .unwrap();
    db::two_factor::disable_totp(&pool, &id).await.unwrap();
    let u = db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert!(!u.totp_enabled);
    assert!(u.totp_secret_encrypted.is_none());
    assert!(u.totp_nonce.is_none());
    assert!(u.totp_recovery_hashes.is_none());
}

#[tokio::test]
async fn crypto_seal_open_round_trip() {
    let key = [7u8; 32];
    let plaintext = b"top-secret totp seed";
    let (ct, nonce) = lets_chat::crypto::seal(&key, plaintext).unwrap();
    assert_ne!(ct.as_slice(), plaintext);
    let recovered = lets_chat::crypto::open(&key, &nonce, &ct).unwrap();
    assert_eq!(recovered, plaintext);
}

#[tokio::test]
async fn crypto_open_with_wrong_key_fails() {
    let key1 = [1u8; 32];
    let key2 = [2u8; 32];
    let (ct, nonce) = lets_chat::crypto::seal(&key1, b"data").unwrap();
    assert!(lets_chat::crypto::open(&key2, &nonce, &ct).is_err());
}

/// Common 2FA bug: the base32 string shown to the user under "Can't scan? Use
/// this secret" drifts from the secret embedded in the QR otpauth URL. They
/// must come from the same source. This test rebuilds a TOTP with the same
/// parameters the route handler uses and asserts the two strings are equal.
#[tokio::test]
async fn qr_secret_matches_displayed_secret() {
    use totp_rs::{Algorithm, Secret, TOTP};

    let raw = Secret::generate_secret().to_bytes().unwrap();
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        raw.clone(),
        Some("lets-chat".to_string()),
        "alice".to_string(),
    )
    .unwrap();

    let displayed = totp.get_secret_base32();
    let url = totp.get_url();

    let qr_secret = url
        .split_once("?secret=")
        .and_then(|(_, rest)| rest.split('&').next())
        .expect("otpauth url must carry secret= param");

    assert_eq!(
        qr_secret, displayed,
        "QR-embedded secret must match the manual-entry string"
    );

    let manual_via_lib = totp_rs::Secret::Encoded(displayed.clone())
        .to_bytes()
        .unwrap();
    assert_eq!(manual_via_lib, raw, "round-trip back to the original bytes");
}
