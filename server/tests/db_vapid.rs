use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use lets_chat::db::vapid;
use sqlx::SqlitePool;

async fn setup_settings_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/settings/0001_create_tables.sql"),
        include_str!("../migrations/settings/0002_uploads.sql"),
        include_str!("../migrations/settings/0003_vapid_keypair.sql"),
        include_str!("../migrations/settings/0004_anti_spam.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn test_secret_key() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"vapid-test-key");
    let out = h.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&out);
    k
}

#[tokio::test]
async fn first_call_generates_and_persists() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let kp = vapid::load_or_generate(&pool, &key).await.unwrap();
    let raw = URL_SAFE_NO_PAD.decode(&kp.public_key_b64url).unwrap();
    assert_eq!(raw.len(), 65);
    assert_eq!(raw[0], 0x04);
    assert_eq!(kp.private_key_bytes.len(), 32);
}

#[tokio::test]
async fn second_call_returns_persisted_keypair() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let first = vapid::load_or_generate(&pool, &key).await.unwrap();
    let second = vapid::load_or_generate(&pool, &key).await.unwrap();
    assert_eq!(first.public_key_b64url, second.public_key_b64url);
    assert_eq!(first.private_key_bytes, second.private_key_bytes);
}

#[tokio::test]
async fn private_key_is_not_stored_plaintext() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let kp = vapid::load_or_generate(&pool, &key).await.unwrap();
    let row: (Vec<u8>,) =
        sqlx::query_as("SELECT private_key_encrypted FROM vapid_keypair WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    // Encrypted blob must not contain the raw 32 bytes verbatim.
    assert!(
        !row.0
            .windows(kp.private_key_bytes.len())
            .any(|w| w == kp.private_key_bytes.as_slice()),
        "encrypted blob should not contain the raw key bytes"
    );
}

#[tokio::test]
async fn wrong_key_fails_to_decrypt() {
    let pool = setup_settings_pool().await;
    let key = test_secret_key();
    let _ = vapid::load_or_generate(&pool, &key).await.unwrap();
    let mut wrong = key;
    wrong[0] ^= 0xff;
    assert!(vapid::load_or_generate(&pool, &wrong).await.is_err());
}
