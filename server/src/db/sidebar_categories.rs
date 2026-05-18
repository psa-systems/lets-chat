//! Per-enclave room categorization (LC-79 redesign).
//!
//! Categories and assignments live in `chat.db`: they are part of the
//! enclave's shared state, managed by the enclave's admin / moderator /
//! owner, and visible to all members of that enclave. The per-user
//! collapsed flag (private "fold this category in my own sidebar"
//! preference) lives in `auth.db`'s `collapsed_categories` table and
//! is fetched separately by the sidebar loader.
//!
//! Module name kept as `sidebar_categories` for diff readability; the
//! underlying tables (`room_categories`, `room_category_assignments`)
//! tell the new story.
use std::collections::{HashMap, HashSet};

use sqlx::{Row, SqlitePool};

/// Metadata for a single category in one enclave. The `rooms` field is
/// intentionally NOT included; callers join against [`room_assignments_for_enclave`]
/// to bucket rooms per category at render time.
pub struct RoomCategory {
    pub id: i64,
    pub enclave_id: i64,
    pub name: String,
    pub position: i64,
}

/// List every category owned by `enclave_id`, in admin-controlled
/// position order. Position is sparse but stable; new categories get
/// `max + 1`.
pub async fn list_categories_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<RoomCategory>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, enclave_id, name, position \
           FROM room_categories \
          WHERE enclave_id = ? \
          ORDER BY position ASC, id ASC",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RoomCategory {
            id: r.get("id"),
            enclave_id: r.get("enclave_id"),
            name: r.get("name"),
            position: r.get("position"),
        })
        .collect())
}

/// Insert a new category in `enclave_id`. Position defaults to one past
/// the current max in that enclave so new categories land at the
/// bottom. Returns the new category id. Caller (route layer) is
/// responsible for the admin/mod/owner RBAC check.
pub async fn create_category(
    pool: &SqlitePool,
    enclave_id: i64,
    name: &str,
) -> Result<i64, sqlx::Error> {
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM room_categories WHERE enclave_id = ?",
    )
    .bind(enclave_id)
    .fetch_one(pool)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO room_categories (enclave_id, name, position) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(enclave_id)
    .bind(name)
    .bind(next_position)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Rename a category. Scoped to `enclave_id` so a spoofed id from a
/// different enclave is a no-op. Row count of 0 = "not yours / doesn't
/// exist", route layer turns into 404.
pub async fn rename_category(
    pool: &SqlitePool,
    enclave_id: i64,
    category_id: i64,
    name: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE room_categories SET name = ? WHERE id = ? AND enclave_id = ?")
        .bind(name)
        .bind(category_id)
        .bind(enclave_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Delete a category. `ON DELETE CASCADE` on
/// `room_category_assignments.category_id` drops every assignment to
/// this category in the same transaction; the rooms themselves are
/// untouched.
pub async fn delete_category(
    pool: &SqlitePool,
    enclave_id: i64,
    category_id: i64,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM room_categories WHERE id = ? AND enclave_id = ?")
        .bind(category_id)
        .bind(enclave_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Move one room into one category. A room belongs to at most one
/// category (PK on `room_id`), so an existing row is replaced.
/// Position defaults to one past the current max within the target
/// category. The caller (route layer) verifies that the room belongs
/// to the same enclave as the category.
pub async fn assign_room(
    pool: &SqlitePool,
    room_id: i64,
    category_id: i64,
) -> Result<(), sqlx::Error> {
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 \
           FROM room_category_assignments \
          WHERE category_id = ?",
    )
    .bind(category_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO room_category_assignments (room_id, category_id, position) \
         VALUES (?, ?, ?) \
         ON CONFLICT (room_id) DO UPDATE SET \
            category_id = excluded.category_id, \
            position = excluded.position",
    )
    .bind(room_id)
    .bind(category_id)
    .bind(next_position)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a room from whatever category it is in. The room itself
/// stays in its enclave; it just falls back to the uncategorized
/// "All rooms" bucket.
pub async fn unassign_room(pool: &SqlitePool, room_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM room_category_assignments WHERE room_id = ?")
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Apply a new ordering for the categories in `enclave_id`. Wraps every
/// per-id UPDATE in a transaction so a partial failure leaves the
/// existing order untouched.
pub async fn set_category_positions(
    pool: &SqlitePool,
    enclave_id: i64,
    category_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (idx, cat_id) in category_ids.iter().enumerate() {
        sqlx::query("UPDATE room_categories SET position = ? WHERE enclave_id = ? AND id = ?")
            .bind(idx as i64)
            .bind(enclave_id)
            .bind(cat_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply a new ordering for rooms inside one category. Handles three
/// shapes: same-category reorder, cross-category drag (room currently
/// assigned to a different category gets upserted), and newly
/// categorizing (room previously uncategorized lands here). Caller
/// (route layer) verifies room access and enclave consistency.
pub async fn set_room_positions(
    pool: &SqlitePool,
    category_id: i64,
    room_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (idx, room_id) in room_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO room_category_assignments (room_id, category_id, position) \
             VALUES (?, ?, ?) \
             ON CONFLICT (room_id) DO UPDATE SET \
                category_id = excluded.category_id, \
                position = excluded.position",
        )
        .bind(room_id)
        .bind(category_id)
        .bind(idx as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `room_id -> (category_id, position)` for every room currently
/// assigned to a category in `enclave_id`. The inner JOIN scopes the
/// result to one enclave so the sidebar bucketing only sees relevant
/// rows.
pub async fn room_assignments_for_enclave(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<HashMap<i64, (i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.room_id, a.category_id, a.position \
           FROM room_category_assignments a \
           INNER JOIN room_categories c ON c.id = a.category_id \
          WHERE c.enclave_id = ?",
    )
    .bind(enclave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("room_id"),
                (r.get::<i64, _>("category_id"), r.get::<i64, _>("position")),
            )
        })
        .collect())
}

/// Look up the enclave a category belongs to. Used by the route layer
/// to RBAC-check against the right enclave.
pub async fn enclave_of_category(
    pool: &SqlitePool,
    category_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<i64> =
        sqlx::query_scalar("SELECT enclave_id FROM room_categories WHERE id = ?")
            .bind(category_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// Per-user collapsed-state helpers (auth.db side). Each user can
/// independently fold/expand any category. Existence in
/// `collapsed_categories` means collapsed; absence means expanded.
pub async fn list_collapsed_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashSet<i64>, sqlx::Error> {
    let rows = sqlx::query("SELECT category_id FROM collapsed_categories WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<i64, _>("category_id"))
        .collect())
}

pub async fn set_collapsed(
    pool: &SqlitePool,
    user_id: &str,
    category_id: i64,
    collapsed: bool,
) -> Result<(), sqlx::Error> {
    if collapsed {
        sqlx::query(
            "INSERT OR IGNORE INTO collapsed_categories (user_id, category_id) VALUES (?, ?)",
        )
        .bind(user_id)
        .bind(category_id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM collapsed_categories WHERE user_id = ? AND category_id = ?")
            .bind(user_id)
            .bind(category_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}
