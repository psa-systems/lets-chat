//! AES-256-GCM wrapper for OIDC client secrets stored in `sso_providers`.
//!
//! Wraps the per-byte-array primitives in `crate::crypto` into a
//! single-blob storage format: 12-byte nonce prepended to the
//! ciphertext+tag. The `sso_providers.client_secret_encrypted` column
//! holds the concatenation as one BLOB rather than splitting nonce
//! into its own column, since the admin UI only ever round-trips the
//! whole secret as an opaque value.
//!
//! Key source is the same `LETS_CHAT_SECRET_KEY` that already protects
//! the SMTP password and TOTP secrets via `crate::crypto`. When the
//! key is unset, the admin-create route refuses with 503 before this
//! module is reached.

use crate::crypto::{self, CryptoError};

const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("encrypt failed")]
    Encrypt,
    #[error("decrypt failed")]
    Decrypt,
    #[error("ciphertext blob is shorter than the {NONCE_LEN}-byte nonce prefix")]
    Truncated,
    #[error("decrypted secret is not valid UTF-8")]
    NotUtf8,
}

impl From<CryptoError> for SecretError {
    fn from(value: CryptoError) -> Self {
        match value {
            CryptoError::Encrypt => SecretError::Encrypt,
            CryptoError::Decrypt => SecretError::Decrypt,
        }
    }
}

/// Encrypt a client secret for storage in `sso_providers.client_secret_encrypted`.
/// Output is `nonce || ciphertext` as a single byte vector.
pub fn encrypt_client_secret(key: &[u8; 32], plaintext: &str) -> Result<Vec<u8>, SecretError> {
    let (ciphertext, nonce) = crypto::seal(key, plaintext.as_bytes())?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Reverse of `encrypt_client_secret`. Splits the nonce prefix, decrypts
/// the rest, and returns the secret as the UTF-8 string it was before
/// sealing.
pub fn decrypt_client_secret(key: &[u8; 32], blob: &[u8]) -> Result<String, SecretError> {
    if blob.len() < NONCE_LEN {
        return Err(SecretError::Truncated);
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let plaintext = crypto::open(key, nonce, ciphertext)?;
    String::from_utf8(plaintext).map_err(|_| SecretError::NotUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, byte) in k.iter_mut().enumerate() {
            *byte = i as u8;
        }
        k
    }

    #[test]
    fn round_trip_matches_input() {
        let blob = encrypt_client_secret(&key(), "super-secret-value").unwrap();
        let plain = decrypt_client_secret(&key(), &blob).unwrap();
        assert_eq!(plain, "super-secret-value");
    }

    #[test]
    fn encrypted_output_differs_from_plaintext() {
        let blob = encrypt_client_secret(&key(), "abc").unwrap();
        assert!(!blob.windows(3).any(|w| w == b"abc"));
    }

    #[test]
    fn two_seals_use_different_nonces() {
        let a = encrypt_client_secret(&key(), "same").unwrap();
        let b = encrypt_client_secret(&key(), "same").unwrap();
        assert_ne!(&a[..NONCE_LEN], &b[..NONCE_LEN]);
        assert_ne!(a, b);
    }

    #[test]
    fn truncated_blob_errors() {
        let err = decrypt_client_secret(&key(), &[0u8; 8]).unwrap_err();
        assert!(matches!(err, SecretError::Truncated));
    }

    #[test]
    fn wrong_key_errors() {
        let blob = encrypt_client_secret(&key(), "abc").unwrap();
        let mut other = key();
        other[0] ^= 0xff;
        let err = decrypt_client_secret(&other, &blob).unwrap_err();
        assert!(matches!(err, SecretError::Decrypt));
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let blob = encrypt_client_secret(&key(), "").unwrap();
        let plain = decrypt_client_secret(&key(), &blob).unwrap();
        assert_eq!(plain, "");
    }
}
