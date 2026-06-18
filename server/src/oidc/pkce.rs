//! LC-22: PKCE primitives + opaque-token helpers.

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

const B64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// 32 random bytes, base64url encoded. Suitable for `state` and `nonce`. The
/// 43-character result satisfies the OIDC spec's recommended entropy.
pub fn new_random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64URL.encode(bytes)
}

/// `(code_verifier, code_challenge)` for S256 PKCE. The verifier is 43 chars
/// (32 random bytes base64url-encoded); the challenge is the SHA-256 of the
/// verifier, base64url-encoded.
pub fn new_pkce_pair() -> (String, String) {
    let verifier = new_random_token();
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = B64URL.encode(hasher.finalize());
    (verifier, challenge)
}
