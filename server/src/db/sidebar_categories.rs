//! Per-user sidebar categorization of rooms.
//!
//! Categorization is private: each user organises their own sidebar without
//! affecting anyone else's. Both tables live in `auth.db` (alongside the
//! user record) rather than `chat.db` so the data follows the user, not the
//! room. The actual room metadata (name, unread, mute) is still resolved
//! out of `chat.db` by the existing sidebar loader; this module only owns
//! the (user, room) -> category mapping.
use std::collections::HashMap;

use sqlx::{Row, SqlitePool};

/// Metadata for a single category in a user's sidebar. The `rooms` field
/// is intentionally NOT included; callers join against [`room_assignments`]
/// (or the per-room map it produces) so the category list and the room
/// metadata stay independent.
pub struct SidebarCategory {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub collapsed: bool,
}

/// List every category owned by `user_id`, in user-controlled position
/// order. Position is sparse but stable; new categories get `max + 1`.
pub async fn list_categories(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<SidebarCategory>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, position, collapsed \
           FROM sidebar_categories \
          WHERE user_id = ? \
          ORDER BY position ASC, id ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SidebarCategory {
            id: r.get("id"),
            name: r.get("name"),
            position: r.get("position"),
            collapsed: r.get::<i64, _>("collapsed") != 0,
        })
        .collect())
}

/// Insert a new category for `user_id`. Position defaults to one past the
/// current max so new categories land at the bottom of the sidebar.
/// Returns the new category id.
pub async fn create_category(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
) -> Result<i64, sqlx::Error> {
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM sidebar_categories WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sidebar_categories (user_id, name, position) \
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(next_position)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Rename a category. The `user_id` filter prevents one user from
/// renaming another user's category even if the route handler is given a
/// stale id; row count of 0 = "not yours / doesn't exist", route layer
/// turns that into 404.
pub async fn rename_category(
    pool: &SqlitePool,
    user_id: &str,
    category_id: i64,
    name: &str,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE sidebar_categories SET name = ? WHERE id = ? AND user_id = ?")
        .bind(name)
        .bind(category_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Toggle the collapsed flag for one category. Same `user_id` guard as
/// rename.
pub async fn set_collapsed(
    pool: &SqlitePool,
    user_id: &str,
    category_id: i64,
    collapsed: bool,
) -> Result<u64, sqlx::Error> {
    let res =
        sqlx::query("UPDATE sidebar_categories SET collapsed = ? WHERE id = ? AND user_id = ?")
            .bind(if collapsed { 1 } else { 0 })
            .bind(category_id)
            .bind(user_id)
            .execute(pool)
            .await?;
    Ok(res.rows_affected())
}

/// Delete a category. `ON DELETE CASCADE` on `sidebar_category_rooms`
/// drops all room assignments to this category in the same transaction,
/// so the affected rooms fall back to the "All rooms" bucket on the next
/// sidebar render. Rooms themselves are untouched.
pub async fn delete_category(
    pool: &SqlitePool,
    user_id: &str,
    category_id: i64,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM sidebar_categories WHERE id = ? AND user_id = ?")
        .bind(category_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Move one room into one category. A room can belong to at most one
/// category per user, so an existing row is replaced. Position defaults
/// to one past the current max within the target category. The caller
/// (route layer) is responsible for verifying that `user_id` is actually
/// a member of `room_id` in `chat.db`; this function trusts its inputs.
pub async fn assign_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    category_id: i64,
) -> Result<(), sqlx::Error> {
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), -1) + 1 \
           FROM sidebar_category_rooms \
          WHERE category_id = ?",
    )
    .bind(category_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO sidebar_category_rooms (user_id, room_id, category_id, position) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (user_id, room_id) DO UPDATE SET \
            category_id = excluded.category_id, \
            position = excluded.position",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(category_id)
    .bind(next_position)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a room from whatever category it is in. The room itself stays
/// joined; it just falls back to the uncategorized "All rooms" bucket.
/// Also called automatically when a user leaves a room (see
/// [`forget_room`]).
pub async fn unassign_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sidebar_category_rooms WHERE user_id = ? AND room_id = ?")
        .bind(user_id)
        .bind(room_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Same as [`unassign_room`] but expresses intent: "user left the room,
/// scrub all sidebar-categorization state for them". Kept as a separate
/// name so the call site at the room-leave handler reads correctly.
pub async fn forget_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
) -> Result<(), sqlx::Error> {
    unassign_room(pool, user_id, room_id).await
}

/// Bulk variant for the enclave-leave / enclave-kick path. The handler
/// resolves the list of room ids the user is losing access to (via the
/// chat.db `list_rooms_in_enclave` query), then this function drops
/// every matching assignment row for that user in one statement. Empty
/// slice is a no-op (no SQL fired); supports up to ~999 ids per call
/// (sqlite's variable limit), more than any realistic enclave.
pub async fn forget_rooms(
    pool: &SqlitePool,
    user_id: &str,
    room_ids: &[i64],
) -> Result<(), sqlx::Error> {
    if room_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", room_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "DELETE FROM sidebar_category_rooms WHERE user_id = ? AND room_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql).bind(user_id);
    for id in room_ids {
        q = q.bind(id);
    }
    q.execute(pool).await?;
    Ok(())
}

/// Apply a new ordering for the categories owned by `user_id`. The
/// caller passes the full list of category ids in the desired order;
/// this rewrites `position` for each as its index in the list, scoped
/// to the user so a stale or spoofed id from a different user is a
/// no-op rather than a privilege escalation. Categories not present in
/// the list are left untouched.
pub async fn set_category_positions(
    pool: &SqlitePool,
    user_id: &str,
    category_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (idx, cat_id) in category_ids.iter().enumerate() {
        sqlx::query("UPDATE sidebar_categories SET position = ? WHERE user_id = ? AND id = ?")
            .bind(idx as i64)
            .bind(user_id)
            .bind(cat_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply a new ordering for rooms inside one category. Handles three
/// shapes in one call:
///   - same-category reorder: room already assigned to this category
///     gets a new position;
///   - cross-category drag: room currently assigned to a DIFFERENT
///     category is upserted into this one at the new position;
///   - newly categorizing: room previously uncategorized lands in this
///     category at the new position.
///
/// Wrapped in a transaction so a partial failure leaves the assignment
/// table untouched. The caller is responsible for verifying that the
/// user is actually a member of every room id passed in; this function
/// trusts its inputs.
pub async fn set_room_positions(
    pool: &SqlitePool,
    user_id: &str,
    category_id: i64,
    room_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (idx, room_id) in room_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO sidebar_category_rooms (user_id, room_id, category_id, position) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT (user_id, room_id) DO UPDATE SET \
                category_id = excluded.category_id, \
                position = excluded.position",
        )
        .bind(user_id)
        .bind(room_id)
        .bind(category_id)
        .bind(idx as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Per-user map of `room_id -> (category_id, position)` so the sidebar
/// renderer can bucket each room into the right category in O(rooms)
/// rather than running one query per room.
pub async fn room_assignments(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<HashMap<i64, (i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT room_id, category_id, position \
           FROM sidebar_category_rooms \
          WHERE user_id = ?",
    )
    .bind(user_id)
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
