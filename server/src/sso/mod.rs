//! SSO (OIDC RP). Sign-in via an external Identity Provider, coexisting
//! with the existing username/password auth.
//!
//! v1 supports a single configured IdP (mokosh-server in the PSA suite).
//! The internal state shape is BYO-SSO-ready: providers are held in a
//! map keyed by an opaque provider id; today there's at most one entry
//! under the key `"default"`. See `docs/lets-chat/sso/09-byo-sso-architecture.md`
//! for the future shape.
//!
//! This module is **config + state-shape only** in L2. No routes, no
//! DB helpers, no OIDC client yet. Those layers come in L3 / L4 / L5+.

use std::collections::HashMap;

mod config;
pub mod secret;

pub use config::{SsoConfig, DEFAULT_PROVIDER_ID};

/// Container for all configured SSO providers. Today holds at most one
/// entry (under `"default"`); BYO-SSO support extends this to N entries
/// keyed by per-provider opaque slugs. Cloneable via `Arc`-shared
/// inner state when threading through `AppState`.
#[derive(Debug, Clone, Default)]
pub struct SsoProviders {
    inner: HashMap<&'static str, SsoConfig>,
}

impl SsoProviders {
    /// Insert a provider under the given opaque slug id. The slug is
    /// arbitrary today (v1 uses "default"); BYO-SSO will use per-IdP
    /// slugs like "mokosh", "google", "microsoft". Replaces an
    /// existing entry under the same id.
    pub fn register(&mut self, id: &'static str, cfg: SsoConfig) {
        self.inner.insert(id, cfg);
    }

    /// True when at least one provider is configured. Routes check
    /// this at mount time; UI checks it to decide whether to render
    /// the "Sign in with SSO" button.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Look up a provider by id. `None` when the id isn't configured.
    /// The v1 single-provider code path looks up `DEFAULT_PROVIDER_ID`.
    pub fn lookup(&self, provider_id: &str) -> Option<&SsoConfig> {
        self.inner.get(provider_id)
    }

    /// Iterate every configured (id, config) pair. Used by the future
    /// login-page renderer to list one button per provider.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &SsoConfig)> {
        self.inner.iter().map(|(k, v)| (*k, v))
    }
}
