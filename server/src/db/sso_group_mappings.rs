//! SQLx helpers for `sso_group_mappings`. Wired into the L17 sync at
//! sign-in and the L17 admin-UI section that lets operators edit the
//! per-provider mapping table.

use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMappingRow {
    pub id: i64,
    pub provider_id: String,
    pub group_value: String,
    pub enclave_id: i64,
    pub role: String,
    pub created_at: i64,
}

/// All mappings for one provider, ordered by `group_value` then
/// `enclave_id` for a stable list-view render.
pub async fn list_for_provider(
    pool: &SqlitePool,
    provider_id: &str,
) -> Result<Vec<GroupMappingRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, provider_id, group_value, enclave_id, role, created_at \
         FROM sso_group_mappings WHERE provider_id = ? \
         ORDER BY group_value COLLATE NOCASE ASC, enclave_id ASC",
    )
    .bind(provider_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| GroupMappingRow {
            id: r.get("id"),
            provider_id: r.get("provider_id"),
            group_value: r.get("group_value"),
            enclave_id: r.get("enclave_id"),
            role: r.get("role"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// All mappings matching one provider + group-value pair. The L17
/// sign-in sync calls this once per group the IdP listed for the user.
pub async fn list_for_group(
    pool: &SqlitePool,
    provider_id: &str,
    group_value: &str,
) -> Result<Vec<GroupMappingRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, provider_id, group_value, enclave_id, role, created_at \
         FROM sso_group_mappings WHERE provider_id = ? AND group_value = ?",
    )
    .bind(provider_id)
    .bind(group_value)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| GroupMappingRow {
            id: r.get("id"),
            provider_id: r.get("provider_id"),
            group_value: r.get("group_value"),
            enclave_id: r.get("enclave_id"),
            role: r.get("role"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Insert one mapping. Returns `Conflict`-shaped sqlx error on UNIQUE
/// collision so the route layer can surface a friendly message.
pub async fn insert(
    pool: &SqlitePool,
    provider_id: &str,
    group_value: &str,
    enclave_id: i64,
    role: &str,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO sso_group_mappings (provider_id, group_value, enclave_id, role) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(provider_id)
    .bind(group_value)
    .bind(enclave_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Delete one mapping by primary key. Returns the number of rows
/// removed (0 when the id doesn't exist).
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sso_group_mappings WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Replace the role on an existing mapping. The (provider, group,
/// enclave) triple is locked; only the granted role is editable in
/// place. Returns rows affected (0 when the id doesn't exist).
pub async fn update_role(pool: &SqlitePool, id: i64, role: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE sso_group_mappings SET role = ? WHERE id = ?")
        .bind(role)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Drop every mapping for the given provider. Useful for the admin UI
/// "clear all" action and as a defensive cleanup if a provider is
/// re-created with the same slug after a delete (cascade already
/// handles the normal delete path).
pub async fn delete_all_for_provider(
    pool: &SqlitePool,
    provider_id: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sso_group_mappings WHERE provider_id = ?")
        .bind(provider_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
