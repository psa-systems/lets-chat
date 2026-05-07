use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("encrypt failed")]
    Encrypt,
    #[error("decrypt failed")]
    Decrypt,
}

pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Encrypt)?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    Ok((ciphertext, nonce_bytes.to_vec()))
}

pub fn open(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if nonce.len() != 12 {
        return Err(CryptoError::Decrypt);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Decrypt)?;
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decrypt)
}

/// Returns `None` when `LETS_CHAT_SECRET_KEY` is unset or empty. Callers
/// must treat absent key as "two-factor authentication disabled" rather than
/// inventing an ephemeral key, since TOTP secrets sealed under a key that
/// vanishes on restart would lock every user out.
pub fn load_secret_key_from_env() -> Option<[u8; 32]> {
    match std::env::var("LETS_CHAT_SECRET_KEY") {
        Ok(s) if !s.is_empty() => Some(derive_key_from_string(&s)),
        _ => {
            tracing::warn!("LETS_CHAT_SECRET_KEY not set; two-factor authentication is disabled.");
            None
        }
    }
}

fn derive_key_from_string(s: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    let mut k = [0u8; 32];
    k.copy_from_slice(&out);
    k
}
