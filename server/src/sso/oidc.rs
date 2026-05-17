//! Token exchange + id_token verification for the SSO callback.
//!
//! Two helpers callable from the callback route:
//!   * [`exchange_code`] POSTs the authorization code to the IdP's
//!     `token_endpoint` and returns the parsed id_token + access_token.
//!   * [`verify_id_token`] verifies the id_token JWT against the JWKS
//!     served by the IdP, then runs the OIDC claim checks (iss, aud,
//!     exp, nonce) and returns the subject + email + name + groups
//!     claims callers need.
//!
//! Signature algorithms supported in v1: RS256, ES256, HS256. RS256 is
//! the OIDC default; ES256 is what Apple + some recent IdPs use; HS256
//! is rare but appears in test fixtures so it's worth handling rather
//! than refusing the sign-in with a confusing error.

use std::time::Duration;

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use reqwest::Client as HttpClient;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("network error during token exchange: {0}")]
    Network(#[from] reqwest::Error),
    #[error("token endpoint returned {status}: {body}")]
    TokenStatus { status: u16, body: String },
    #[error("malformed token-endpoint response: {0}")]
    BadTokenJson(#[from] serde_json::Error),
    #[error("id_token absent from token response")]
    MissingIdToken,
    #[error("id_token has no `kid` and JWKS has multiple keys; cannot disambiguate")]
    AmbiguousKid,
    #[error("no JWK matched the id_token's kid `{0}`")]
    UnknownKid(String),
    #[error("JWKS could not be parsed: {0}")]
    BadJwks(serde_json::Error),
    #[error("id_token header could not be decoded: {0}")]
    BadHeader(jsonwebtoken::errors::Error),
    #[error("id_token signature / claims verification failed: {0}")]
    Verify(jsonwebtoken::errors::Error),
    #[error("id_token alg `{0:?}` is not supported")]
    UnsupportedAlg(Algorithm),
}

/// Subset of the token-endpoint response we care about.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: Option<String>,
    pub id_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_in: Option<i64>,
}

/// Exchange an authorization code for tokens. Uses
/// `application/x-www-form-urlencoded`; that's what the spec mandates
/// even though most IdPs would happily accept JSON.
pub async fn exchange_code(
    http: &HttpClient,
    token_endpoint: &url::Url,
    code: &str,
    pkce_verifier: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, OidcError> {
    let res = http
        .post(token_endpoint.as_str())
        .timeout(Duration::from_secs(10))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", pkce_verifier),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;
    let status = res.status();
    let body = res.text().await?;
    if !status.is_success() {
        return Err(OidcError::TokenStatus {
            status: status.as_u16(),
            body,
        });
    }
    Ok(serde_json::from_str(&body)?)
}

/// OIDC id_token claims relevant to the sign-in path.
#[derive(Debug, Clone, Deserialize)]
pub struct IdTokenClaims {
    /// OIDC subject. Stable id assigned by the IdP; the load-bearing
    /// link key in `sso_identities`.
    pub sub: String,
    /// OIDC email claim. Per-provider attribute_map can override which
    /// claim name to read from; defaulting here is fine because the
    /// caller projects the raw map onto the IdTokenClaims fields after
    /// the parse via `apply_attribute_map` (L15) - for L11 we read the
    /// default fields directly.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    /// Groups claim, when the IdP emits one. Empty / absent in most
    /// vanilla deployments. L17 consumes this for enclave sync.
    #[serde(default)]
    pub groups: Option<Vec<String>>,
}

/// Verify the id_token signature against the JWKS, run the OIDC claim
/// checks (iss + aud + exp), and verify the nonce echo. Returns the
/// parsed claims on success.
pub fn verify_id_token(
    id_token: &str,
    jwks_json: &str,
    expected_issuer: &str,
    expected_audience: &str,
    expected_nonce: &str,
) -> Result<IdTokenClaims, OidcError> {
    let jwks: JwkSet = serde_json::from_str(jwks_json).map_err(OidcError::BadJwks)?;
    let header = jsonwebtoken::decode_header(id_token).map_err(OidcError::BadHeader)?;
    let jwk = match (&header.kid, jwks.keys.as_slice()) {
        (Some(kid), keys) => keys
            .iter()
            .find(|k| k.common.key_id.as_deref() == Some(kid.as_str()))
            .ok_or_else(|| OidcError::UnknownKid(kid.clone()))?,
        (None, [single]) => single,
        (None, _) => return Err(OidcError::AmbiguousKid),
    };
    let key = decoding_key_for(jwk)?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[expected_issuer]);
    validation.set_audience(&[expected_audience]);
    // Default `exp` validation is on; we explicitly add `nbf` + `iat`
    // leeway so a 30s clock skew doesn't reject an otherwise valid
    // token. OIDC core leaves the leeway up to the RP.
    validation.leeway = 30;

    #[derive(Debug, Deserialize)]
    struct WireClaims {
        sub: String,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        email_verified: Option<bool>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        preferred_username: Option<String>,
        #[serde(default)]
        groups: Option<Vec<String>>,
        nonce: Option<String>,
    }

    let token = jsonwebtoken::decode::<WireClaims>(id_token, &key, &validation)
        .map_err(OidcError::Verify)?;
    let claims = token.claims;
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(OidcError::Verify(jsonwebtoken::errors::Error::from(
            jsonwebtoken::errors::ErrorKind::InvalidToken,
        )));
    }
    Ok(IdTokenClaims {
        sub: claims.sub,
        email: claims.email,
        email_verified: claims.email_verified,
        name: claims.name,
        preferred_username: claims.preferred_username,
        groups: claims.groups,
    })
}

fn decoding_key_for(jwk: &Jwk) -> Result<DecodingKey, OidcError> {
    use jsonwebtoken::jwk::AlgorithmParameters;
    match &jwk.algorithm {
        AlgorithmParameters::RSA(rsa) => {
            DecodingKey::from_rsa_components(&rsa.n, &rsa.e).map_err(OidcError::Verify)
        }
        AlgorithmParameters::EllipticCurve(ec) => {
            DecodingKey::from_ec_components(&ec.x, &ec.y).map_err(OidcError::Verify)
        }
        AlgorithmParameters::OctetKey(oct) => {
            use base64::engine::general_purpose::URL_SAFE_NO_PAD;
            use base64::Engine as _;
            let secret = URL_SAFE_NO_PAD.decode(&oct.value).map_err(|_| {
                OidcError::Verify(jsonwebtoken::errors::Error::from(
                    jsonwebtoken::errors::ErrorKind::Base64(base64::DecodeError::InvalidLength(0)),
                ))
            })?;
            Ok(DecodingKey::from_secret(&secret))
        }
        other => Err(OidcError::UnsupportedAlg(match other {
            AlgorithmParameters::OctetKeyPair(_) => Algorithm::EdDSA,
            _ => Algorithm::HS256,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn make_hs256_jwt(secret: &[u8], kid: Option<&str>, claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::HS256);
        if let Some(k) = kid {
            header.kid = Some(k.to_string());
        }
        encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
    }

    fn jwks_with_oct(kid: &str, secret: &[u8]) -> String {
        json!({
            "keys": [{
                "kty": "oct",
                "kid": kid,
                "alg": "HS256",
                "k": URL_SAFE_NO_PAD.encode(secret),
            }]
        })
        .to_string()
    }

    #[test]
    fn verify_happy_path_returns_claims() {
        let secret = b"shhh-test-secret";
        let kid = "k1";
        let token = make_hs256_jwt(
            secret,
            Some(kid),
            json!({
                "iss": "https://idp/",
                "aud": "client-1",
                "exp": now_secs() + 600,
                "sub": "user-42",
                "email": "alice@example.com",
                "email_verified": true,
                "name": "Alice",
                "preferred_username": "alice",
                "nonce": "the-nonce",
            }),
        );
        let claims = verify_id_token(
            &token,
            &jwks_with_oct(kid, secret),
            "https://idp/",
            "client-1",
            "the-nonce",
        )
        .unwrap();
        assert_eq!(claims.sub, "user-42");
        assert_eq!(claims.email.as_deref(), Some("alice@example.com"));
        assert_eq!(claims.email_verified, Some(true));
        assert_eq!(claims.name.as_deref(), Some("Alice"));
        assert_eq!(claims.preferred_username.as_deref(), Some("alice"));
    }

    #[test]
    fn verify_rejects_wrong_audience() {
        let secret = b"s";
        let token = make_hs256_jwt(
            secret,
            Some("k"),
            json!({
                "iss": "https://idp/",
                "aud": "different-client",
                "exp": now_secs() + 600,
                "sub": "u",
                "nonce": "n",
            }),
        );
        let err = verify_id_token(
            &token,
            &jwks_with_oct("k", secret),
            "https://idp/",
            "client-1",
            "n",
        )
        .unwrap_err();
        assert!(matches!(err, OidcError::Verify(_)));
    }

    #[test]
    fn verify_rejects_wrong_nonce() {
        let secret = b"s";
        let token = make_hs256_jwt(
            secret,
            Some("k"),
            json!({
                "iss": "https://idp/",
                "aud": "client-1",
                "exp": now_secs() + 600,
                "sub": "u",
                "nonce": "wrong",
            }),
        );
        let err = verify_id_token(
            &token,
            &jwks_with_oct("k", secret),
            "https://idp/",
            "client-1",
            "expected",
        )
        .unwrap_err();
        assert!(matches!(err, OidcError::Verify(_)));
    }

    #[test]
    fn verify_rejects_expired_token() {
        let secret = b"s";
        let token = make_hs256_jwt(
            secret,
            Some("k"),
            json!({
                "iss": "https://idp/",
                "aud": "client-1",
                "exp": now_secs() - 600,
                "sub": "u",
                "nonce": "n",
            }),
        );
        let err = verify_id_token(
            &token,
            &jwks_with_oct("k", secret),
            "https://idp/",
            "client-1",
            "n",
        )
        .unwrap_err();
        assert!(matches!(err, OidcError::Verify(_)));
    }

    #[test]
    fn verify_rejects_unknown_kid() {
        let secret = b"s";
        let token = make_hs256_jwt(
            secret,
            Some("nope"),
            json!({
                "iss": "https://idp/",
                "aud": "client-1",
                "exp": now_secs() + 600,
                "sub": "u",
                "nonce": "n",
            }),
        );
        let err = verify_id_token(
            &token,
            &jwks_with_oct("k", secret),
            "https://idp/",
            "client-1",
            "n",
        )
        .unwrap_err();
        assert!(matches!(err, OidcError::UnknownKid(_)));
    }
}
