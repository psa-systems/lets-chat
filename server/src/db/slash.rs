//! LC-76: admin-defined custom slash commands (`slash_commands_custom`).
//! Built-in commands live in `crate::commands`; these are operator additions
//! resolved at dispatch time. v1 manages the global scope only.

use sqlx::{Row, SqlitePool};

/// Execution kind for a custom command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomKind {
    /// `target` is a template; `{args}` is replaced and posted as a message.
    StaticText,
    /// `target` is a URL; args are POSTed and the response posted as a message.
    WebhookPost,
}

impl CustomKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CustomKind::StaticText => "static_text",
            CustomKind::WebhookPost => "webhook_post",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "static_text" => Some(CustomKind::StaticText),
            "webhook_post" => Some(CustomKind::WebhookPost),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CustomCommand {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub kind: CustomKind,
    pub target: String,
    pub admin_only: bool,
}

fn map_row(r: sqlx::sqlite::SqliteRow) -> CustomCommand {
    CustomCommand {
        id: r.get("id"),
        name: r.get("name"),
        description: r.get("description"),
        // Stored values are constrained by the CHECK; default to static_text
        // on the impossible mismatch rather than panicking.
        kind: CustomKind::parse(&r.get::<String, _>("kind")).unwrap_or(CustomKind::StaticText),
        target: r.get("target"),
        admin_only: r.get::<i64, _>("admin_only") != 0,
    }
}

/// All global custom commands, name-ordered (admin list + autocomplete).
pub async fn list_global(pool: &SqlitePool) -> sqlx::Result<Vec<CustomCommand>> {
    let rows = sqlx::query(
        "SELECT id, name, description, kind, target, admin_only \
         FROM slash_commands_custom WHERE scope_kind = 'global' AND scope_id = 0 \
         ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_row).collect())
}

/// Resolve one global custom command by name (lowercased, no leading slash).
pub async fn get_global(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<CustomCommand>> {
    let row = sqlx::query(
        "SELECT id, name, description, kind, target, admin_only \
         FROM slash_commands_custom WHERE scope_kind = 'global' AND scope_id = 0 AND name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_row))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_global(
    pool: &SqlitePool,
    name: &str,
    description: &str,
    kind: CustomKind,
    target: &str,
    admin_only: bool,
    created_by: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO slash_commands_custom \
            (scope_kind, scope_id, name, description, kind, target, admin_only, created_by) \
         VALUES ('global', 0, ?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(description)
    .bind(kind.as_str())
    .bind(target)
    .bind(admin_only as i64)
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM slash_commands_custom WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// LC-487: per-user canned responses live in `canned_responses` (user-scoped)
// but are surfaced as `CustomCommand`s so the existing slash dispatch +
// autocomplete handle them like personal static_text commands. They are never
// admin-only and always StaticText.

fn map_canned(r: sqlx::sqlite::SqliteRow) -> CustomCommand {
    CustomCommand {
        id: r.get("id"),
        name: r.get("name"),
        description: r.get("description"),
        kind: CustomKind::StaticText,
        target: r.get("body"),
        admin_only: false,
    }
}

/// Every canned response owned by `user_id`, name-ordered.
pub async fn list_canned_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> sqlx::Result<Vec<CustomCommand>> {
    let rows = sqlx::query(
        "SELECT id, name, description, body FROM canned_responses \
         WHERE user_id = ? ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(map_canned).collect())
}

/// Resolve one of `user_id`'s canned responses by name (lowercased, no slash).
pub async fn get_canned_for_user(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
) -> sqlx::Result<Option<CustomCommand>> {
    let row = sqlx::query(
        "SELECT id, name, description, body FROM canned_responses \
         WHERE user_id = ? AND name = ?",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_canned))
}

/// Insert a canned response for `user_id`. Unique per (user, name).
pub async fn insert_canned(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
    description: &str,
    body: &str,
) -> sqlx::Result<i64> {
    let res = sqlx::query(
        "INSERT INTO canned_responses (user_id, name, description, body) VALUES (?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(name)
    .bind(description)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Delete one of `user_id`'s canned responses, scoped to its owner. Returns the
/// number of rows removed (0 when it is not theirs / does not exist).
pub async fn delete_canned(pool: &SqlitePool, user_id: &str, id: i64) -> sqlx::Result<u64> {
    let res = sqlx::query("DELETE FROM canned_responses WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
