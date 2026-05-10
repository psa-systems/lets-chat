//! VAPID keypair persistence for Web Push.
//!
//! On first boot the server generates an ECDSA P-256 keypair, AES-256-GCM
//! encrypts the raw 32-byte private scalar under the process secret key,
//! and writes the singleton row of `vapid_keypair`. On every subsequent
//! boot, the row is loaded and decrypted.
//!
//! Push is disabled entirely when no secret key is configured; this
//! module is therefore only ever called when `secret_key` is `Some`.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use sqlx::{Row, SqlitePool};

use crate::crypto;

/// Decrypted, in-memory VAPID keypair held on `AppState`.
#[derive(Clone)]
pub struct VapidKeypair {
    /// Raw uncompressed P-256 point (65 bytes, leading 0x04), base64url-
    /// encoded. Sent to the page as the `applicationServerKey` value.
    pub public_key_b64url: String,
    /// Raw 32-byte SEC1 private scalar. Loaded into
    /// `jwt_simple::algorithms::ES256KeyPair::from_bytes` when building a
    /// VAPID-signed Push request.
    pub private_key_bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum VapidError {
    #[error("crypto: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("sql: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("p256 key encoding")]
    KeyEncoding,
}

/// Returns the persisted keypair, generating + persisting one on first call.
/// Idempotent: subsequent calls observe the existing row and return the
/// same keypair.
pub async fn load_or_generate(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<VapidKeypair, VapidError> {
    if let Some(kp) = load(pool, secret_key).await? {
        return Ok(kp);
    }
    let kp = generate()?;
    persist(pool, secret_key, &kp).await?;
    Ok(kp)
}

async fn load(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
) -> Result<Option<VapidKeypair>, VapidError> {
    let row = sqlx::query(
        "SELECT public_key_b64url, private_key_encrypted, private_key_nonce \
           FROM vapid_keypair WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let public_key_b64url: String = r.get("public_key_b64url");
    let encrypted: Vec<u8> = r.get("private_key_encrypted");
    let nonce: Vec<u8> = r.get("private_key_nonce");
    let private_key_bytes = crypto::open(secret_key, &nonce, &encrypted)?;
    if private_key_bytes.len() != 32 {
        return Err(VapidError::KeyEncoding);
    }
    Ok(Some(VapidKeypair {
        public_key_b64url,
        private_key_bytes,
    }))
}

async fn persist(
    pool: &SqlitePool,
    secret_key: &[u8; 32],
    kp: &VapidKeypair,
) -> Result<(), VapidError> {
    let (encrypted, nonce) = crypto::seal(secret_key, &kp.private_key_bytes)?;
    sqlx::query(
        "INSERT INTO vapid_keypair \
             (id, public_key_b64url, private_key_encrypted, private_key_nonce) \
         VALUES (1, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&kp.public_key_b64url)
    .bind(&encrypted)
    .bind(&nonce)
    .execute(pool)
    .await?;
    Ok(())
}

/// Generate a fresh ECDSA P-256 keypair. Returns the public key as a
/// base64url-encoded uncompressed point (the `applicationServerKey`
/// format expected by `PushManager.subscribe`) and the private key as
/// the raw 32-byte SEC1 scalar (the format
/// `jwt_simple::algorithms::ES256KeyPair::from_bytes` expects).
fn generate() -> Result<VapidKeypair, VapidError> {
    let signing = p256::SecretKey::random(&mut rand::thread_rng());
    let public_point = signing.public_key().to_encoded_point(false); // 65 bytes uncompressed
    let public_key_b64url = URL_SAFE_NO_PAD.encode(public_point.as_bytes());
    let private_key_bytes = signing.to_bytes().to_vec();
    Ok(VapidKeypair {
        public_key_b64url,
        private_key_bytes,
    })
}
