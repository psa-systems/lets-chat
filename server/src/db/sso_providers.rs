//! SQLx helpers for the `sso_providers` table.
//!
//! Provider rows are the admin-managed equivalent of the L2 env-var
//! single-provider config. See docs/lets-chat/sso/10-admin-managed-providers.md.
//! The admin CRUD routes that call these helpers land in L8; today's
//! callers are the env-var seed path (L6) and the integration tests.

use sqlx::{Row, SqlitePool};

/// One row in `sso_providers`. The plaintext `client_secret` is **not**
/// stored on the struct - callers either pass it in to `insert` /
/// `update` for encryption, or decrypt the stored blob on demand via
/// [`crate::sso::secret::decrypt_client_secret`] when they need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoProviderRow {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret_encrypted: Vec<u8>,
    pub scopes: String,
    pub attribute_map_json: String,
    pub allow_signup: bool,
    pub auto_link_verified_email: bool,
    pub enabled_at: Option<i64>,
    pub disabled_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SsoProviderRow {
    /// True when the provider is currently live. Equivalent to the
    /// `enabled_at IS NOT NULL AND (disabled_at IS NULL OR disabled_at < enabled_at)`
    /// predicate the SQL uses for filtering. Exposed as a struct method
    /// so non-SQL callers (the cache invalidator, the admin list view)
    /// can reuse the same definition.
    pub fn is_enabled(&self) -> bool {
        match (self.enabled_at, self.disabled_at) {
            (Some(_), None) => true,
            (Some(en), Some(dis)) => dis < en,
            _ => false,
        }
    }
}

/// Fields the admin UI hands the insert helper. Plaintext secret is
/// encrypted by the helper before the SQL runs.
pub struct InsertProvider<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub display_name: &'a str,
    pub issuer_url: &'a str,
    pub client_id: &'a str,
    pub client_secret_encrypted: &'a [u8],
    pub scopes: &'a str,
    pub attribute_map_json: &'a str,
    pub allow_signup: bool,
    pub auto_link_verified_email: bool,
}

/// Fields the admin UI hands the update helper. `client_secret_encrypted`
/// is optional so a save-without-rotating-secret round-trips cleanly.
pub struct UpdateProvider<'a> {
    pub display_name: &'a str,
    pub issuer_url: &'a str,
    pub client_id: &'a str,
    pub client_secret_encrypted: Option<&'a [u8]>,
    pub scopes: &'a str,
    pub attribute_map_json: &'a str,
    pub allow_signup: bool,
    pub auto_link_verified_email: bool,
}

fn row_to_provider(r: sqlx::sqlite::SqliteRow) -> SsoProviderRow {
    SsoProviderRow {
        id: r.get("id"),
        kind: r.get("kind"),
        display_name: r.get("display_name"),
        issuer_url: r.get("issuer_url"),
        client_id: r.get("client_id"),
        client_secret_encrypted: r.get("client_secret_encrypted"),
        scopes: r.get("scopes"),
        attribute_map_json: r.get("attribute_map_json"),
        allow_signup: r.get::<i64, _>("allow_signup") != 0,
        auto_link_verified_email: r.get::<i64, _>("auto_link_verified_email") != 0,
        enabled_at: r.get("enabled_at"),
        disabled_at: r.get("disabled_at"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

const SELECT_COLUMNS: &str = "id, kind, display_name, issuer_url, client_id, \
     client_secret_encrypted, scopes, attribute_map_json, \
     allow_signup, auto_link_verified_email, \
     enabled_at, disabled_at, created_at, updated_at";

/// Every provider, enabled or not. Used by the admin list view.
pub async fn list_providers(pool: &SqlitePool) -> Result<Vec<SsoProviderRow>, sqlx::Error> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM sso_providers ORDER BY display_name COLLATE NOCASE ASC"
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(row_to_provider).collect())
}

/// Only currently-enabled providers. Used by the login page and the
/// callback path; both refuse to act on disabled providers.
pub async fn list_enabled_providers(pool: &SqlitePool) -> Result<Vec<SsoProviderRow>, sqlx::Error> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM sso_providers \
         WHERE enabled_at IS NOT NULL \
           AND (disabled_at IS NULL OR disabled_at < enabled_at) \
         ORDER BY display_name COLLATE NOCASE ASC"
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(row_to_provider).collect())
}

pub async fn get_provider_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<SsoProviderRow>, sqlx::Error> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM sso_providers WHERE id = ?");
    let row = sqlx::query(&sql).bind(id).fetch_optional(pool).await?;
    Ok(row.map(row_to_provider))
}

pub async fn get_provider_by_issuer(
    pool: &SqlitePool,
    issuer_url: &str,
) -> Result<Option<SsoProviderRow>, sqlx::Error> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM sso_providers WHERE issuer_url = ?");
    let row = sqlx::query(&sql)
        .bind(issuer_url)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(row_to_provider))
}

/// Insert a brand-new provider. The row lands disabled by default so an
/// operator can fill in the attribute-map / group-mapping / signup flag
/// before flipping it live with `set_provider_enabled`.
pub async fn insert_provider(
    pool: &SqlitePool,
    args: InsertProvider<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sso_providers \
            (id, kind, display_name, issuer_url, client_id, client_secret_encrypted, \
             scopes, attribute_map_json, allow_signup, auto_link_verified_email, \
             enabled_at, disabled_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, \
                 strftime('%s','now'), strftime('%s','now'))",
    )
    .bind(args.id)
    .bind(args.kind)
    .bind(args.display_name)
    .bind(args.issuer_url)
    .bind(args.client_id)
    .bind(args.client_secret_encrypted)
    .bind(args.scopes)
    .bind(args.attribute_map_json)
    .bind(if args.allow_signup { 1 } else { 0 })
    .bind(if args.auto_link_verified_email { 1 } else { 0 })
    .execute(pool)
    .await?;
    Ok(())
}

/// Update an existing provider in place. The slug `id` and `kind` are
/// not editable. The client secret is rotated only when
/// `args.client_secret_encrypted` is `Some` - `None` means "keep what's
/// already in the row," which is how the admin UI's empty-password
/// input round-trips.
pub async fn update_provider(
    pool: &SqlitePool,
    id: &str,
    args: UpdateProvider<'_>,
) -> Result<u64, sqlx::Error> {
    let res = match args.client_secret_encrypted {
        Some(secret) => {
            sqlx::query(
                "UPDATE sso_providers SET \
                    display_name = ?, issuer_url = ?, client_id = ?, \
                    client_secret_encrypted = ?, scopes = ?, attribute_map_json = ?, \
                    allow_signup = ?, auto_link_verified_email = ?, \
                    updated_at = strftime('%s','now') \
                 WHERE id = ?",
            )
            .bind(args.display_name)
            .bind(args.issuer_url)
            .bind(args.client_id)
            .bind(secret)
            .bind(args.scopes)
            .bind(args.attribute_map_json)
            .bind(if args.allow_signup { 1 } else { 0 })
            .bind(if args.auto_link_verified_email { 1 } else { 0 })
            .bind(id)
            .execute(pool)
            .await?
        }
        None => {
            sqlx::query(
                "UPDATE sso_providers SET \
                    display_name = ?, issuer_url = ?, client_id = ?, \
                    scopes = ?, attribute_map_json = ?, \
                    allow_signup = ?, auto_link_verified_email = ?, \
                    updated_at = strftime('%s','now') \
                 WHERE id = ?",
            )
            .bind(args.display_name)
            .bind(args.issuer_url)
            .bind(args.client_id)
            .bind(args.scopes)
            .bind(args.attribute_map_json)
            .bind(if args.allow_signup { 1 } else { 0 })
            .bind(if args.auto_link_verified_email { 1 } else { 0 })
            .bind(id)
            .execute(pool)
            .await?
        }
    };
    Ok(res.rows_affected())
}

/// Set the live/dead flag. `enable=true` stamps `enabled_at = now()` and
/// leaves `disabled_at` alone; `enable=false` stamps `disabled_at = now()`
/// and leaves `enabled_at` alone. The two-column audit lets repeated
/// toggles preserve every flip's timestamp.
pub async fn set_provider_enabled(
    pool: &SqlitePool,
    id: &str,
    enable: bool,
) -> Result<u64, sqlx::Error> {
    let sql = if enable {
        "UPDATE sso_providers SET enabled_at = strftime('%s','now'), \
            updated_at = strftime('%s','now') WHERE id = ?"
    } else {
        "UPDATE sso_providers SET disabled_at = strftime('%s','now'), \
            updated_at = strftime('%s','now') WHERE id = ?"
    };
    let res = sqlx::query(sql).bind(id).execute(pool).await?;
    Ok(res.rows_affected())
}

/// Count `sso_identities` rows still keyed to the given issuer URL.
/// Used by the admin delete route to refuse a delete that would
/// orphan linked users. Returns 0 when no rows reference the issuer.
pub async fn count_identities_for_issuer(
    pool: &SqlitePool,
    issuer_url: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM sso_identities WHERE issuer = ?")
        .bind(issuer_url)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("c"))
}

/// Delete a provider row. Callers should refuse the delete at the
/// route layer when `sso_identities` still references its issuer.
/// This helper is unconditional - no FK enforcement on the SQL side
/// because `sso_identities.issuer` keys by URL text, not by `provider.id`.
pub async fn delete_provider(pool: &SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sso_providers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
