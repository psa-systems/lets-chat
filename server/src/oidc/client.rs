//! LC-22: Bunyip SSO client state.
//!
//! Holds the discovery doc + JWKS cached at startup. Refreshes the JWKS
//! lazily when the callback hits a token with an unknown `kid` (Bunyip key
//! rotation).
//!
//! The client is constructed via `initialize` at startup. A failure here
//! (network, malformed discovery doc, JWKS without an EdDSA key) is fatal
//! and the binary refuses to start. We never want a half-wired SSO surface
//! where the button renders but every callback 502s. See
//! `docs/lets-chat/sso/bunyip-only/04-lets-chat-server-additions.md` §4.2.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::RwLock;

use super::config::BunyipSsoConfig;
use super::verify::{verify_id_token, IdTokenClaims};

const B64URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Debug, Error)]
pub enum BunyipSsoError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("discovery: {0}")]
    Discovery(String),
    #[error("jwks: {0}")]
    Jwks(String),
    #[error("token endpoint: {0}")]
    Token(String),
    #[error("userinfo endpoint: {0}")]
    UserInfo(String),
    #[error("verify: {0}")]
    Verify(String),
}

/// True iff `name` is set to `1` or `true` (case-insensitive). Mirrors the
/// truthy-env convention used by `retention::sweep::flag_enabled`.
fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
    jwks_uri: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    /// Base64url-encoded raw public-key bytes for Ed25519 (`kty=OKP`,
    /// `crv=Ed25519`).
    #[serde(default)]
    x: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JwksDoc {
    keys: Vec<Jwk>,
}

/// Subset of the OIDC `userinfo` response we read. `sub` is the only field
/// the spec guarantees; `email` and `preferred_username` are claims our
/// scope set (`openid email`) should produce when bunyip's user has them.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Subset of the token-endpoint response. We use `id_token` (verified) and
/// `access_token` (for the one-shot userinfo call). `refresh_token` is
/// never requested (we don't ask for `offline_access`) so we don't try to
/// parse it.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub id_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

/// The hot data the runtime client needs to round-trip a callback: the
/// configured RP credentials, the discovered endpoints, and the live JWKS
/// keyed by `kid`. Built once at startup; `keys` is held in an `RwLock` so
/// a lazy rotation can swap it in without restarting the binary.
pub struct BunyipSsoClient {
    pub config: BunyipSsoConfig,
    discovery: Discovery,
    keys: RwLock<Vec<EdKey>>,
    http: Client,
}

#[derive(Debug, Clone)]
struct EdKey {
    kid: Option<String>,
    /// Raw 32-byte Ed25519 public key, base64url-decoded from the JWK `x`
    /// param. Despite the historical field name, this is NOT SPKI-DER:
    /// `jsonwebtoken`'s `DecodingKey::from_ed_der` stores its input verbatim
    /// and the eventual `ring::signature::UnparsedPublicKey::new(ED25519, ..)`
    /// call expects raw 32-byte key material, not SPKI wrapping.
    raw: Vec<u8>,
}

impl BunyipSsoClient {
    /// Fetch discovery + JWKS, build the client. Failure is fatal at
    /// startup.
    pub async fn initialize(config: BunyipSsoConfig) -> Result<Arc<Self>, BunyipSsoError> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("lets-chat/", env!("CARGO_PKG_VERSION")));
        // DEV-ONLY escape hatch. The dev OP (bunyip) sits behind a Traefik that
        // serves a self-signed default cert for *.a8n.run, and reqwest's
        // rustls-tls uses bundled webpki roots that ignore the system CA store,
        // so server-to-server discovery/JWKS/token calls fail TLS verification
        // with no env- or mount-only remedy. When this flag is truthy we accept
        // invalid certs for those calls. Never set it in production: it disables
        // issuer TLS authentication for the entire SSO trust path.
        if env_flag_enabled("LETS_CHAT_BUNYIP_SSO_INSECURE_TLS") {
            tracing::warn!(
                "LETS_CHAT_BUNYIP_SSO_INSECURE_TLS set; accepting invalid TLS certs for Bunyip SSO (dev only)"
            );
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder.build()?;
        let discovery_url = config
            .issuer
            .join("/.well-known/openid-configuration")
            .map_err(|e| BunyipSsoError::Discovery(format!("bad issuer URL: {e}")))?;
        let discovery: Discovery = http
            .get(discovery_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(|e| BunyipSsoError::Discovery(format!("parse: {e}")))?;
        if discovery.issuer.trim_end_matches('/') != config.issuer.as_str().trim_end_matches('/') {
            return Err(BunyipSsoError::Discovery(format!(
                "issuer mismatch: configured={}, advertised={}",
                config.issuer, discovery.issuer
            )));
        }
        let keys = fetch_jwks(&http, &discovery.jwks_uri).await?;
        Ok(Arc::new(Self {
            config,
            discovery,
            keys: RwLock::new(keys),
            http,
        }))
    }

    /// Build the URL we 302 the browser to in `GET /auth/bunyip/start`.
    /// The OIDC dance unpacks: state + nonce in the URL, code_challenge as
    /// S256, our redirect_uri pinned to the seeded oauth_clients row.
    pub fn authorize_url(&self, state: &str, nonce: &str, code_challenge: &str) -> String {
        let mut u = url::Url::parse(&self.discovery.authorization_endpoint)
            .expect("authorize endpoint URL parsed at discovery");
        u.query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid email")
            .append_pair("redirect_uri", self.config.redirect_uri.as_str())
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256");
        u.into()
    }

    /// POST to `/oauth2/token` with `client_secret_basic` auth and the PKCE
    /// verifier from the consumed `oidc_pending` row.
    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<TokenResponse, BunyipSsoError> {
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("code_verifier", code_verifier),
            ("client_id", &self.config.client_id),
        ];
        let resp = self
            .http
            .post(&self.discovery.token_endpoint)
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .form(&form)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(BunyipSsoError::Token(format!(
                "status {status}: {}",
                body.chars().take(500).collect::<String>()
            )));
        }
        resp.json::<TokenResponse>()
            .await
            .map_err(|e| BunyipSsoError::Token(format!("parse: {e}")))
    }

    /// Verify the `id_token` against the cached JWKS, refetching once on
    /// unknown `kid`. The `expected_nonce` MUST match the value stashed in
    /// the consumed `oidc_pending` row.
    pub async fn verify_id_token(
        &self,
        token: &str,
        expected_nonce: &str,
    ) -> Result<IdTokenClaims, BunyipSsoError> {
        let kid = peek_kid(token).map_err(BunyipSsoError::Verify)?;
        let raw = self.lookup_key(kid.as_deref()).await;
        let issuer = self.discovery.issuer.as_str();
        let aud = &self.config.client_id;
        match raw {
            Some(raw) => match verify_id_token(token, &raw, issuer, aud, expected_nonce) {
                Ok(c) => Ok(c),
                Err(_) => {
                    // Refresh JWKS once and retry. Covers the
                    // bunyip-side key rotation scenario.
                    self.refresh_jwks().await?;
                    let raw = self
                        .lookup_key(kid.as_deref())
                        .await
                        .ok_or_else(|| BunyipSsoError::Verify("unknown kid".into()))?;
                    verify_id_token(token, &raw, issuer, aud, expected_nonce)
                        .map_err(BunyipSsoError::Verify)
                }
            },
            None => {
                self.refresh_jwks().await?;
                let raw = self
                    .lookup_key(kid.as_deref())
                    .await
                    .ok_or_else(|| BunyipSsoError::Verify("unknown kid".into()))?;
                verify_id_token(token, &raw, issuer, aud, expected_nonce)
                    .map_err(BunyipSsoError::Verify)
            }
        }
    }

    /// GET `/oauth2/userinfo` with the access token. Returns the parsed
    /// subject + email + preferred_username.
    pub async fn user_info(&self, access_token: &str) -> Result<UserInfo, BunyipSsoError> {
        let endpoint =
            self.discovery.userinfo_endpoint.as_deref().ok_or_else(|| {
                BunyipSsoError::UserInfo("userinfo_endpoint not advertised".into())
            })?;
        let resp = self
            .http
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(BunyipSsoError::UserInfo(format!(
                "status {status}: {}",
                body.chars().take(500).collect::<String>()
            )));
        }
        resp.json::<UserInfo>()
            .await
            .map_err(|e| BunyipSsoError::UserInfo(format!("parse: {e}")))
    }

    async fn lookup_key(&self, kid: Option<&str>) -> Option<Vec<u8>> {
        let keys = self.keys.read().await;
        match kid {
            Some(k) => keys
                .iter()
                .find(|key| key.kid.as_deref() == Some(k))
                .map(|key| key.raw.clone()),
            // No kid in the token header: try the first cached key.
            None => keys.first().map(|key| key.raw.clone()),
        }
    }

    async fn refresh_jwks(&self) -> Result<(), BunyipSsoError> {
        let fresh = fetch_jwks(&self.http, &self.discovery.jwks_uri).await?;
        let mut w = self.keys.write().await;
        *w = fresh;
        Ok(())
    }
}

async fn fetch_jwks(http: &Client, url: &str) -> Result<Vec<EdKey>, BunyipSsoError> {
    let doc: JwksDoc = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .map_err(|e| BunyipSsoError::Jwks(format!("parse: {e}")))?;
    let mut out = Vec::with_capacity(doc.keys.len());
    for k in doc.keys {
        if k.kty != "OKP" || k.crv.as_deref() != Some("Ed25519") {
            continue;
        }
        if let Some(alg) = &k.alg {
            if alg != "EdDSA" {
                continue;
            }
        }
        let Some(x) = k.x else { continue };
        let raw = B64URL
            .decode(x.as_bytes())
            .map_err(|e| BunyipSsoError::Jwks(format!("bad x: {e}")))?;
        if raw.len() != 32 {
            return Err(BunyipSsoError::Jwks(format!(
                "Ed25519 JWK x is {} bytes; expected 32",
                raw.len()
            )));
        }
        out.push(EdKey { kid: k.kid, raw });
    }
    if out.is_empty() {
        return Err(BunyipSsoError::Jwks("no EdDSA keys in JWKS".into()));
    }
    Ok(out)
}

/// Pull the `kid` from the token's protected header without validating the
/// signature. Used to pick which cached key to verify against.
fn peek_kid(token: &str) -> Result<Option<String>, String> {
    let header_b64 = token
        .split('.')
        .next()
        .ok_or("token has no header segment")?;
    let header_bytes = B64URL
        .decode(header_b64.as_bytes())
        .map_err(|e| format!("header b64: {e}"))?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).map_err(|e| format!("header json: {e}"))?;
    Ok(header
        .get("kid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}
