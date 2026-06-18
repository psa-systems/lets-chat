//! LC-22: ID-token verification.
//!
//! We verify EdDSA-only (bunyip mints with EdDSA). The token is verified
//! against the JWKS cache the client holds; an unknown `kid` triggers a
//! one-shot JWKS re-fetch (handled in `client.rs`).

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: AudClaim,
    pub exp: i64,
    pub iat: Option<i64>,
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
}

/// `aud` can be either a string or an array of strings per RFC 7519. We
/// accept either shape and normalize to a `Vec<String>` for the audience
/// check downstream.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AudClaim {
    One(String),
    Many(Vec<String>),
}

impl AudClaim {
    pub fn contains(&self, expected: &str) -> bool {
        match self {
            AudClaim::One(s) => s == expected,
            AudClaim::Many(v) => v.iter().any(|s| s == expected),
        }
    }
}

/// Verify the EdDSA signature + standard claims. The `nonce` argument is the
/// value the SSO start handler stashed in the `oidc_pending` row; we require
/// it to match the `nonce` claim in the token to prevent replay.
pub fn verify_id_token(
    token: &str,
    jwk_der: &[u8],
    expected_issuer: &str,
    expected_audience: &str,
    expected_nonce: &str,
) -> Result<IdTokenClaims, String> {
    let key = DecodingKey::from_ed_der(jwk_der);
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_issuer(&[expected_issuer]);
    // `jsonwebtoken` enforces `aud` membership; we set it to the configured
    // client_id and let the library handle it.
    validation.set_audience(&[expected_audience]);
    validation.leeway = 60;
    // Our `IdTokenClaims` uses a custom `aud` shape but `jsonwebtoken`
    // verifies against its own deserialization for the audience check. We
    // therefore decode twice: once with the library's audience check
    // (rejected if `aud != expected_audience`), once into our shape for the
    // nonce / sub / email / preferred_username fields.
    let data = jsonwebtoken::decode::<IdTokenClaims>(token, &key, &validation)
        .map_err(|e| format!("id_token verification failed: {e}"))?;
    let claims = data.claims;
    if !claims.aud.contains(expected_audience) {
        return Err("id_token aud does not match client_id".into());
    }
    match &claims.nonce {
        Some(n) if n == expected_nonce => {}
        Some(_) => return Err("id_token nonce mismatch".into()),
        None => return Err("id_token missing nonce".into()),
    }
    Ok(claims)
}
