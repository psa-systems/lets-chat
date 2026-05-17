//! Per-provider cache entries: pair the DB row with its lazily-fetched
//! OIDC discovery metadata. Held inside [`super::SsoProviders`].
//!
//! Each entry's `discovery` field is a `tokio::sync::OnceCell` that
//! fills on the first sign-in attempt for the provider. Subsequent
//! attempts reuse the cached metadata for the lifetime of the entry.
//! Admin writes (insert/update/delete/toggle) replace the entry via
//! [`super::SsoProviders::reload`], which drops the cached metadata so
//! the next sign-in re-discovers - the cheapest correct invalidation.

use std::sync::Arc;

use reqwest::Client as HttpClient;
use tokio::sync::OnceCell;

use crate::db::sso_providers::SsoProviderRow;
use crate::sso::discovery::{self, DiscoveryError, DiscoveryMetadata};

#[derive(Debug)]
pub struct ProviderEntry {
    pub row: SsoProviderRow,
    discovery: OnceCell<Arc<DiscoveryMetadata>>,
}

impl ProviderEntry {
    pub fn new(row: SsoProviderRow) -> Self {
        Self {
            row,
            discovery: OnceCell::new(),
        }
    }

    /// Return the cached discovery metadata, fetching it on the first
    /// call. Concurrent calls coalesce into one network round-trip via
    /// `OnceCell::get_or_try_init`.
    pub async fn discovery(
        &self,
        http: &HttpClient,
    ) -> Result<Arc<DiscoveryMetadata>, DiscoveryError> {
        self.discovery
            .get_or_try_init(|| async {
                let issuer = url::Url::parse(&self.row.issuer_url).map_err(|source| {
                    DiscoveryError::BadUrl {
                        field: "issuer_url",
                        source,
                    }
                })?;
                discovery::discover(&issuer, http).await.map(Arc::new)
            })
            .await
            .cloned()
    }

    /// `Some` when discovery has been resolved at least once. Used by
    /// tests + future admin-UI affordances ("Last discovered: ...").
    pub fn discovery_cached(&self) -> Option<Arc<DiscoveryMetadata>> {
        self.discovery.get().cloned()
    }
}
