//! Per-enclave user groups (LC-83). A group lives inside one enclave;
//! the mention parser expands `@group-name` to the group's member set
//! at send time, so each member gets a per-user `mentions` row through
//! the existing pipeline.
use sqlx::{Row, SqlitePool};

pub struct UserGroup {
    pub id: i64,
    pub enclave_id: i64,
    pub name: String,
    pub description: Option<String>,
}

pub async fn create(
    pool: &SqlitePool,
    enclave_id: i64,
    name: &str,
    description: Option<&str>,
    created_by: &str,
) -> Result<i64, sqlx::Error> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO user_groups (enclave_id, name, description, created_by) \
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(enclave_id)
    .bind(name)
    .bind(description)
    .bind(created_by)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn rename(
    pool: &SqlitePool,
    enclave_id: i64,
    group_id: i64,
    name: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE user_groups SET name = ? WHERE id = ? AND enclave_id = ?")
        .bind(name)
        .bind(group_id)
        .bind(enclave_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn delete(pool: &SqlitePool, enclave_id: i64, group_id: i64) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM user_groups WHERE id = ? AND enclave_id = ?")
        .bind(group_id)
        .bind(enclave_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn list_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<UserGroup>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, enclave_id, name, description \
           FROM user_groups WHERE enclave_id = ? ORDER BY name",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| UserGroup {
            id: r.get("id"),
            enclave_id: r.get("enclave_id"),
            name: r.get("name"),
            description: r.get("description"),
        })
        .collect())
}

/// Resolve `@token` against the groups in `enclave_id`. Returns the
/// matched group's id when the token is the exact (case-sensitive) name
/// of an existing group; `None` otherwise. Used by the mention parser
/// to decide whether to expand to member ids.
pub async fn find_by_name(
    pool: &SqlitePool,
    enclave_id: i64,
    name: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<i64> =
        sqlx::query_scalar("SELECT id FROM user_groups WHERE enclave_id = ? AND name = ?")
            .bind(enclave_id)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

pub async fn add_member(
    pool: &SqlitePool,
    group_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO user_group_members (group_id, user_id) VALUES (?, ?)")
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_member(
    pool: &SqlitePool,
    group_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_group_members WHERE group_id = ? AND user_id = ?")
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_member_ids(pool: &SqlitePool, group_id: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id FROM user_group_members WHERE group_id = ?")
        .bind(group_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("user_id"))
        .collect())
}
