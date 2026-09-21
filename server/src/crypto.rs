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

/// LC-1004: a `LETS_CHAT_SECRET_KEY` that is set but guessable. The value is
/// never included in the message: `.env.standalone` ships a placeholder, so a
/// match here is public by definition, but echoing the variable at all invites
/// the next edit to echo a real one (LC-1003).
#[derive(Debug, thiserror::Error)]
pub enum SecretKeyError {
    #[error("LETS_CHAT_SECRET_KEY is a known placeholder; generate one with: openssl rand -base64 32")]
    Placeholder,
    #[error("LETS_CHAT_SECRET_KEY is {len} characters, at least {min} are required; generate one with: openssl rand -base64 32")]
    TooShort { len: usize, min: usize },
    #[error("LETS_CHAT_SECRET_KEY repeats a single character; generate one with: openssl rand -base64 32")]
    NoEntropy,
}

/// LC-1004: what the templates ship, plus the stand-ins people type instead.
/// `.env.standalone:66` ships the first one active rather than commented, so
/// the insecure value is the shipped default, not an operator slip.
const SECRET_KEY_PLACEHOLDERS: [&str; 6] = [
    "change-me-in-production",
    "changeme",
    "change-me",
    "secret",
    "password",
    "test",
];

/// LC-1004: `openssl rand -base64 32`, which both templates recommend, yields
/// 44 characters. A floor of 16 rejects a typed word without rejecting anything
/// a generator produces.
const SECRET_KEY_MIN_LEN: usize = 16;

/// LC-1004: reject a present-but-guessable key. Deliberately blunt (deny list
/// plus a length floor) rather than an entropy estimate, which is easy to get
/// wrong in both directions and invites argument about the threshold.
fn validate_secret_key(s: &str) -> Result<(), SecretKeyError> {
    let trimmed = s.trim();
    if SECRET_KEY_PLACEHOLDERS.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return Err(SecretKeyError::Placeholder);
    }
    let len = trimmed.chars().count();
    if len < SECRET_KEY_MIN_LEN {
        return Err(SecretKeyError::TooShort { len, min: SECRET_KEY_MIN_LEN });
    }
    let mut chars = trimmed.chars();
    if let Some(first) = chars.next() {
        if chars.all(|c| c == first) {
            return Err(SecretKeyError::NoEntropy);
        }
    }
    Ok(())
}

/// Returns `Ok(None)` when `LETS_CHAT_SECRET_KEY` is unset or empty. Callers
/// must treat absent key as "two-factor authentication disabled" rather than
/// inventing an ephemeral key, since TOTP secrets sealed under a key that
/// vanishes on restart would lock every user out.
///
/// LC-1004: a key that is present but guessable is an `Err`, not `Ok(None)`.
/// Degrading to the absent case would silently disable two-factor
/// authentication and leave anything already sealed unreadable, which is the
/// worse and quieter failure; the caller refuses to start instead.
pub fn load_secret_key_from_env() -> Result<Option<[u8; 32]>, SecretKeyError> {
    let raw = match std::env::var("LETS_CHAT_SECRET_KEY") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            tracing::warn!("LETS_CHAT_SECRET_KEY not set; two-factor authentication is disabled.");
            return Ok(None);
        }
    };
    validate_secret_key(&raw)?;
    Ok(Some(derive_key_from_string(&raw)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_the_shipped_placeholder_in_any_case_or_padding() {
        for v in [
            "change-me-in-production",
            "CHANGE-ME-IN-PRODUCTION",
            "  change-me-in-production  ",
        ] {
            assert!(matches!(validate_secret_key(v), Err(SecretKeyError::Placeholder)), "{v:?}");
        }
    }

    #[test]
    fn rejects_the_other_stand_ins() {
        for v in ["changeme", "change-me", "secret", "password", "test"] {
            assert!(matches!(validate_secret_key(v), Err(SecretKeyError::Placeholder)), "{v:?}");
        }
    }

    #[test]
    fn rejects_a_value_under_the_length_floor() {
        assert!(matches!(
            validate_secret_key("abc123"),
            Err(SecretKeyError::TooShort { len: 6, min: SECRET_KEY_MIN_LEN })
        ));
    }

    #[test]
    fn rejects_a_single_repeated_character_that_clears_the_floor() {
        assert!(matches!(validate_secret_key(&"a".repeat(40)), Err(SecretKeyError::NoEntropy)));
    }

    #[test]
    fn accepts_a_key_that_clears_every_rule() {
        // Built rather than written out: a literal with the shape of
        // `openssl rand -base64 32` is exactly what a secret scanner is built
        // to catch, and committing one to test a secret guard would trip the
        // repository's own gate (LC-977). The rules care about length and
        // variety, not encoding, so a synthetic run of characters exercises
        // the accept path just as well.
        let generated: String = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
        assert!(generated.chars().count() >= SECRET_KEY_MIN_LEN);
        assert!(validate_secret_key(&generated).is_ok());
    }
}
