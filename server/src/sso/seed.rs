//! Startup seed path: convert the L2 `LETS_CHAT_SSO_*` env vars into a
//! `sso_providers` row when the table is empty for that issuer.
//!
//! Operators who already manage providers through the admin UI never set
//! the env vars; the seed is a no-op for them. Operators upgrading from
//! L2 keep their existing env-var config working: on first boot after
//! the upgrade, the env values land in the table as the `"default"`
//! provider; subsequent edits to that row happen through the admin UI
//! and the env vars are ignored from then on (the DB wins).
//!
//! See docs/lets-chat/sso/10-admin-managed-providers.md section 1.

use crate::db::sso_providers::{self, InsertProvider};
use crate::sso::config::{SsoConfig, DEFAULT_PROVIDER_ID};
use crate::sso::secret::{self, SecretError};

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error(
        "LETS_CHAT_SSO_ISSUER is set but LETS_CHAT_SECRET_KEY is not; \
         the client secret cannot be encrypted without it"
    )]
    NoSecretKey,
    #[error("failed to encrypt seeded client secret: {0}")]
    Encrypt(#[from] SecretError),
    #[error("database error during seed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Insert one row into `sso_providers` under the slug
/// `DEFAULT_PROVIDER_ID` when:
///   1. `SsoConfig::from_env()` returned a config, AND
///   2. No row already exists with the same `issuer_url`.
///
/// Returns `Ok(true)` when a row was inserted, `Ok(false)` when the seed
/// was skipped (env unset, or a row for that issuer already exists).
/// The inserted row lands ENABLED so the upgrade path keeps SSO
/// working without the operator having to click anything in the admin
/// UI.
pub async fn seed_default_from_env(
    pool: &sqlx::SqlitePool,
    secret_key: Option<&[u8; 32]>,
    cfg: &SsoConfig,
) -> Result<bool, SeedError> {
    let issuer_str = cfg.issuer.as_str();
    if sso_providers::get_provider_by_issuer(pool, issuer_str)
        .await?
        .is_some()
    {
        tracing::info!(
            issuer = issuer_str,
            "SSO env vars set but provider row already exists; honouring DB row, ignoring env"
        );
        return Ok(false);
    }
    let key = secret_key.ok_or(SeedError::NoSecretKey)?;
    let encrypted = secret::encrypt_client_secret(key, &cfg.client_secret)?;
    sso_providers::insert_provider(
        pool,
        InsertProvider {
            id: DEFAULT_PROVIDER_ID,
            kind: "oidc",
            display_name: "SSO",
            issuer_url: issuer_str,
            client_id: &cfg.client_id,
            client_secret_encrypted: &encrypted,
            scopes: "openid email profile",
            attribute_map_json: "{}",
            allow_signup: cfg.autoprovision,
            auto_link_verified_email: true,
        },
    )
    .await?;
    // New provider rows are inserted disabled by default; the env-var
    // upgrade path is the one case where we want them live immediately,
    // since the operator's existing deployment had SSO enabled before
    // the upgrade.
    sso_providers::set_provider_enabled(pool, DEFAULT_PROVIDER_ID, true).await?;
    tracing::info!(
        issuer = issuer_str,
        provider_id = DEFAULT_PROVIDER_ID,
        "seeded sso_providers row from LETS_CHAT_SSO_* env vars"
    );
    Ok(true)
}
