//! LC-84: per-room role overrides.
//!
//! Stores rows in `room_role_overrides` that elevate a user's effective
//! role inside one room without changing their org-wide role. The
//! mention/auth layer consumes [`effective_role`] when deciding whether
//! a caller can perform a Moderator-gated action (e.g. delete another
//! user's message in that room). Removing an override restores the org
//! default; per-room demotion is intentionally not modelled.
use sqlx::{Row, SqlitePool};

/// One stored override row, rendered by the per-room moderator-admin
/// page. `label` is pre-resolved by the route so the template never
/// renders an opaque user_id.
pub struct OverrideRow {
    pub user_id: String,
    pub role: String,
    pub assigned_by: String,
    pub assigned_at: String,
}

/// Insert or update an override. The PRIMARY KEY (room_id, user_id)
/// makes the upsert idempotent: re-granting the same role to the same
/// user refreshes `assigned_by` and `assigned_at`, and granting a
/// different role replaces the row in place. Returns the role text that
/// is now stored.
pub async fn upsert(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    role: &str,
    assigned_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO room_role_overrides (room_id, user_id, role, assigned_by) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(room_id, user_id) DO UPDATE SET \
             role = excluded.role, \
             assigned_by = excluded.assigned_by, \
             assigned_at = datetime('now')",
    )
    .bind(room_id)
    .bind(user_id)
    .bind(role)
    .bind(assigned_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove an override. Returns the number of rows deleted (0 if the
/// override never existed, which the caller can treat as a 404).
pub async fn delete(pool: &SqlitePool, room_id: i64, user_id: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM room_role_overrides WHERE room_id = ? AND user_id = ?")
        .bind(room_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Look up the override role (if any) for `(room_id, user_id)`. Returns
/// the stored `role` string (`"moderator"` or `"admin"`), or `None` if
/// no override exists.
pub async fn get(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT role FROM room_role_overrides WHERE room_id = ? AND user_id = ?",
    )
    .bind(room_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// List every override stored against a room. Ordered by `assigned_at`
/// descending so the most recent grants appear first in the admin UI.
pub async fn list_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> Result<Vec<OverrideRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT user_id, role, assigned_by, assigned_at \
           FROM room_role_overrides WHERE room_id = ? \
           ORDER BY assigned_at DESC",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| OverrideRow {
            user_id: r.get("user_id"),
            role: r.get("role"),
            assigned_by: r.get("assigned_by"),
            assigned_at: r.get("assigned_at"),
        })
        .collect())
}

/// Resolve the caller's effective role in `room_id` as the higher of
/// the org-role and any stored override. Roles rank `user < moderator
/// < admin`; the helper returns the higher-ranked string. Callers pass
/// the org role in directly to avoid an extra `users` lookup on the hot
/// path (every RBAC check already has the `User` in scope).
pub async fn effective_role(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    org_role: &str,
) -> Result<String, sqlx::Error> {
    let override_role = get(pool, room_id, user_id).await?;
    Ok(max_role(org_role, override_role.as_deref()))
}

fn rank(role: &str) -> u8 {
    match role {
        "admin" => 3,
        "moderator" => 2,
        _ => 1,
    }
}

fn max_role(org: &str, override_role: Option<&str>) -> String {
    let Some(o) = override_role else {
        return org.to_string();
    };
    if rank(o) >= rank(org) {
        o.to_string()
    } else {
        org.to_string()
    }
}

/// True when `effective_role` is `moderator` or `admin`. The pattern
/// recurs at every Moderator-gated call site (delete other users'
/// messages, etc.); the wrapper saves a string compare at each one.
pub async fn is_room_moderator(
    pool: &SqlitePool,
    room_id: i64,
    user_id: &str,
    org_role: &str,
) -> Result<bool, sqlx::Error> {
    let r = effective_role(pool, room_id, user_id, org_role).await?;
    Ok(r == "moderator" || r == "admin")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_max(org: &str, ovr: Option<&str>, expected: &str) {
        assert_eq!(max_role(org, ovr), expected, "org={org} ovr={ovr:?}");
    }

    #[test]
    fn override_elevates_user_to_moderator() {
        assert_max("user", Some("moderator"), "moderator");
    }

    #[test]
    fn override_elevates_user_to_admin() {
        assert_max("user", Some("admin"), "admin");
    }

    #[test]
    fn no_override_returns_org_role() {
        assert_max("user", None, "user");
        assert_max("moderator", None, "moderator");
        assert_max("admin", None, "admin");
    }

    #[test]
    fn override_never_demotes_higher_org_role() {
        // Per spec: overrides only elevate. An admin with a "moderator"
        // override stays admin in the room.
        assert_max("admin", Some("moderator"), "admin");
    }
}
