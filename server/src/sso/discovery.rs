//! OIDC discovery: fetch `/.well-known/openid-configuration` + its JWKS.
//!
//! Pulled lazily on the first sign-in attempt for a provider, then
//! cached in the [`ProviderEntry`](super::cache::ProviderEntry) for the
//! lifetime of the cache entry. The admin write paths invalidate the
//! cache after a provider edit so the next sign-in re-discovers.
//!
//! We use `reqwest` directly here rather than the `openidconnect`
//! crate's `CoreProviderMetadata::discover_async`: this module owns
//! only the metadata (URLs + raw JWKS bytes); the typed `openidconnect`
//! client is constructed per-request at sign-in time (L9+) from these
//! fields. Keeping the cache plain JSON-shaped means it can survive
//! openidconnect-crate version bumps without a cache-shape change.

use std::time::Duration;

use reqwest::Client as HttpClient;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone)]
pub struct DiscoveryMetadata {
    /// Echoed back so the `iss` claim check on the id_token can run
    /// against the discovery value, not the configured value (mokosh
    /// happens to match, but generic OIDC IdPs sometimes serve discovery
    /// at one URL and announce a different `issuer` claim).
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub userinfo_endpoint: Option<Url>,
    pub jwks_uri: Url,
    /// Verbatim JSON returned by the `jwks_uri` GET. Parsed at
    /// id_token-verify time, not here, because key rotation makes the
    /// parsed form stale faster than the metadata.
    pub jwks_json: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("network error fetching {what}: {source}")]
    Network {
        what: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("non-success status {status} fetching {what}")]
    BadStatus {
        what: &'static str,
        status: reqwest::StatusCode,
    },
    #[error("malformed JSON in {what}: {source}")]
    BadJson {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("malformed URL in discovery document field `{field}`: {source}")]
    BadUrl {
        field: &'static str,
        #[source]
        source: url::ParseError,
    },
}

#[derive(Deserialize)]
struct DiscoveryDoc {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
    jwks_uri: String,
}

/// Fetch and parse `{issuer_url}/.well-known/openid-configuration`,
/// then the `jwks_uri` it points at.
///
/// Trims the issuer URL of any trailing slash to avoid the double-slash
/// in the discovery URL that some IdPs reject.
pub async fn discover(
    issuer_url: &Url,
    http: &HttpClient,
) -> Result<DiscoveryMetadata, DiscoveryError> {
    // Join handles the trailing-slash story for us: a join base whose
    // path doesn't end in `/` discards the last segment (so we ensure
    // it does), and the relative URL must not start with `/` (so it
    // appends rather than replacing the path).
    let mut base = issuer_url.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let discovery_url = base
        .join(".well-known/openid-configuration")
        .map_err(|source| DiscoveryError::BadUrl {
            field: "issuer_url",
            source,
        })?;

    let doc: DiscoveryDoc = fetch_json(http, discovery_url.as_str(), "discovery").await?;

    let authorization_endpoint = parse_url(&doc.authorization_endpoint, "authorization_endpoint")?;
    let token_endpoint = parse_url(&doc.token_endpoint, "token_endpoint")?;
    let userinfo_endpoint = doc
        .userinfo_endpoint
        .as_deref()
        .map(|s| parse_url(s, "userinfo_endpoint"))
        .transpose()?;
    let jwks_uri = parse_url(&doc.jwks_uri, "jwks_uri")?;

    let jwks_json = fetch_text(http, jwks_uri.as_str(), "jwks").await?;

    Ok(DiscoveryMetadata {
        issuer: doc.issuer,
        authorization_endpoint,
        token_endpoint,
        userinfo_endpoint,
        jwks_uri,
        jwks_json,
    })
}

fn parse_url(s: &str, field: &'static str) -> Result<Url, DiscoveryError> {
    Url::parse(s).map_err(|source| DiscoveryError::BadUrl { field, source })
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    http: &HttpClient,
    url: &str,
    what: &'static str,
) -> Result<T, DiscoveryError> {
    let body = fetch_text(http, url, what).await?;
    serde_json::from_str(&body).map_err(|source| DiscoveryError::BadJson { what, source })
}

async fn fetch_text(
    http: &HttpClient,
    url: &str,
    what: &'static str,
) -> Result<String, DiscoveryError> {
    let res = http
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|source| DiscoveryError::Network { what, source })?;
    let status = res.status();
    if !status.is_success() {
        return Err(DiscoveryError::BadStatus { what, status });
    }
    res.text()
        .await
        .map_err(|source| DiscoveryError::Network { what, source })
}
