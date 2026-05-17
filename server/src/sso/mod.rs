//! SSO (OIDC RP). Sign-in via an external Identity Provider, coexisting
//! with the existing username/password auth.
//!
//! Providers are admin-managed: each row in `sso_providers` is one IdP
//! the deployment trusts. The L2 env vars now act as a one-shot seed
//! into that table on startup (see `sso::seed`) so operators upgrading
//! from the single-provider env config keep working without manual
//! re-entry. Future edits happen through the admin UI (L8) and the
//! env vars stay quiet.
//!
//! See `docs/lets-chat/sso/10-admin-managed-providers.md`.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::RwLock;

use crate::db::sso_providers::{self, SsoProviderRow};

mod config;
pub mod secret;
pub mod seed;

pub use config::{SsoConfig, SsoConfigError, DEFAULT_PROVIDER_ID};

/// In-memory cache of every enabled provider row from `sso_providers`.
/// Populated at boot via [`SsoProviders::load_enabled`] and reloaded by
/// the admin write paths (insert/update/delete/toggle). Routes look
/// providers up by id; the login page enumerates them via `iter`.
///
/// Cheap to clone: the inner `HashMap` lives behind an `Arc<RwLock>`.
#[derive(Debug, Clone, Default)]
pub struct SsoProviders {
    inner: Arc<RwLock<HashMap<String, SsoProviderRow>>>,
}

impl SsoProviders {
    /// Populate from the database. Reads every row where
    /// `enabled_at IS NOT NULL AND (disabled_at IS NULL OR disabled_at < enabled_at)`.
    pub async fn load_enabled(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let rows = sso_providers::list_enabled_providers(pool).await?;
        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            map.insert(row.id.clone(), row);
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
        })
    }

    /// True when zero enabled providers are loaded. Drives
    /// `AppState::sso_available()` (login page renders the SSO buttons
    /// only when this is false).
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Snapshot every (id, row) pair. Cloned so the read lock isn't
    /// held across the caller's iteration. Used by the login-page
    /// renderer to list one button per provider.
    pub async fn snapshot(&self) -> Vec<(String, SsoProviderRow)> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Look up a provider by slug. Cloned out of the lock for the
    /// same reason as `snapshot`.
    pub async fn lookup(&self, provider_id: &str) -> Option<SsoProviderRow> {
        self.inner.read().await.get(provider_id).cloned()
    }

    /// Replace the cache from a fresh DB read. Called by the admin
    /// write paths after they mutate `sso_providers`.
    pub async fn reload(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let rows = sso_providers::list_enabled_providers(pool).await?;
        let mut guard = self.inner.write().await;
        guard.clear();
        for row in rows {
            guard.insert(row.id.clone(), row);
        }
        Ok(())
    }
}
