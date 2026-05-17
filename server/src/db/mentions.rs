use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};

/// Username characters we accept inside an `@token`. The auth layer is
/// permissive about what it accepts at registration time, so this is a
/// best-effort token shape; final resolution is by exact lookup against
/// `users.username`. The leading `(?:^|\s)` boundary keeps email addresses
/// (`foo@bar.com`) from matching: `bar` is not preceded by start-of-string
/// or whitespace.
const TOKEN_PATTERN: &str = r"(?:^|\s)@([A-Za-z0-9_-]{1,32})";

pub fn parse_mention_tokens(body: &str) -> Vec<String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(TOKEN_PATTERN).expect("valid regex"));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cap in re.captures_iter(body) {
        let token = cap.get(1).unwrap().as_str().to_string();
        if seen.insert(token.clone()) {
            out.push(token);
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct MentionRef {
    pub user_id: String,
    pub username: String,
}

/// Replace the mention set for `message_id` with `targets`. Removes rows for
/// users not in `targets`, inserts rows for users not previously mentioned,
/// preserves `read_at` for users mentioned both before and after.
///
/// Returns `(added, removed)` so the caller can fan out per-user
/// `Mentioned` / `MentionCleared` events.
pub async fn reconcile_mentions(
    pool: &SqlitePool,
    message_id: i64,
    room_id: i64,
    author_user_id: &str,
    targets: &[MentionRef],
) -> Result<(Vec<MentionRef>, Vec<MentionRef>), sqlx::Error> {
    let existing_rows = sqlx::query("SELECT mentioned_user_id FROM mentions WHERE message_id = ?")
        .bind(message_id)
        .fetch_all(pool)
        .await?;
    let existing: HashSet<String> = existing_rows
        .into_iter()
        .map(|r| r.get::<String, _>("mentioned_user_id"))
        .collect();
    let next: HashMap<String, MentionRef> = targets
        .iter()
        .map(|m| (m.user_id.clone(), m.clone()))
        .collect();

    let added: Vec<MentionRef> = next
        .iter()
        .filter(|(id, _)| !existing.contains(*id))
        .map(|(_, m)| m.clone())
        .collect();
    let removed: Vec<MentionRef> = existing
        .iter()
        .filter(|id| !next.contains_key(*id))
        .map(|id| MentionRef {
            user_id: id.clone(),
            username: String::new(),
        })
        .collect();

    for m in &added {
        sqlx::query(
            "INSERT OR IGNORE INTO mentions \
                 (message_id, room_id, mentioned_user_id, author_user_id) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(room_id)
        .bind(&m.user_id)
        .bind(author_user_id)
        .execute(pool)
        .await?;
    }
    for m in &removed {
        sqlx::query("DELETE FROM mentions WHERE message_id = ? AND mentioned_user_id = ?")
            .bind(message_id)
            .bind(&m.user_id)
            .execute(pool)
            .await?;
    }
    Ok((added, removed))
}

/// Bulk-load mentions for a page of messages. Returns a map from message_id
/// to the list of mention refs. Resolves mentioned_user_id -> username via
/// the auth pool. Used by the room/dm route handlers to thread mention chip
/// data through MessageView without N+1 queries.
pub async fn mentions_for_messages(
    chat_pool: &SqlitePool,
    auth_pool: &SqlitePool,
    message_ids: &[i64],
) -> Result<HashMap<i64, Vec<MentionRef>>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT message_id, mentioned_user_id FROM mentions WHERE message_id IN ({placeholders})"
    );
    let mut q = sqlx::query(&sql);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(chat_pool).await?;

    let mut cache: HashMap<String, String> = HashMap::new();
    let mut by_message: HashMap<i64, Vec<MentionRef>> = HashMap::new();
    for r in rows {
        let mid: i64 = r.get("message_id");
        let uid: String = r.get("mentioned_user_id");
        let username = if let Some(u) = cache.get(&uid) {
            u.clone()
        } else {
            let user = crate::db::auth::find_user_by_id(auth_pool, &uid).await?;
            let name = user.map(|u| u.username).unwrap_or_else(|| uid.clone());
            cache.insert(uid.clone(), name.clone());
            name
        };
        by_message.entry(mid).or_default().push(MentionRef {
            user_id: uid,
            username,
        });
    }
    Ok(by_message)
}

/// Resolve `@username` tokens in `body` against `auth.users.username` and
/// return one MentionRef per match. Used by the edit-history endpoint to
/// render prior bodies: the live-path helper `mentions_for_messages` reads
/// the denormalized `mentions` table, which is reconciled-to-current-body,
/// so a token in a prior body that the live body no longer mentions has no
/// row there to look up. This helper bypasses that table and resolves
/// tokens against `auth.users` directly, so a chip for `@carol` in a prior
/// version still renders even if a later edit removed her from the live
/// body.
///
/// Unresolved tokens (typos, deleted users, `@here` / `@channel` broadcast
/// tokens, users renamed since the edit) are silently dropped; the renderer
/// falls back to literal text for any `@token` not in the returned slice,
/// matching the live-path behavior on the same miss.
pub async fn mentions_for_body(
    auth_pool: &SqlitePool,
    body: &str,
) -> Result<Vec<MentionRef>, sqlx::Error> {
    let tokens = parse_mention_tokens(body);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", tokens.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT id, username FROM users WHERE username IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for t in &tokens {
        q = q.bind(t);
    }
    let rows = q.fetch_all(auth_pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| MentionRef {
            user_id: r.get::<String, _>("id"),
            username: r.get::<String, _>("username"),
        })
        .collect())
}

/// Mark every mention of `user_id` in `room_id` with `message_id <= watermark`
/// as read. Called from the same path as `set_last_read`. Returns the number
/// of rows that flipped from unread to read so the caller can decide whether
/// to broadcast a sidebar refresh.
pub async fn mark_mentions_read_for_room(
    pool: &SqlitePool,
    user_id: &str,
    room_id: i64,
    watermark: i64,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE mentions \
            SET read_at = datetime('now') \
          WHERE mentioned_user_id = ? \
            AND room_id = ? \
            AND message_id <= ? \
            AND read_at IS NULL",
    )
    .bind(user_id)
    .bind(room_id)
    .bind(watermark)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Per-room unread mention counts for the sidebar. Returns rows where
/// `count > 0`. Used by `routes::load_sidebar` to set
/// `SidebarRoom::mentions`.
pub async fn count_unread_mentions_per_room(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT room_id, COUNT(*) AS n \
           FROM mentions \
          WHERE mentioned_user_id = ? AND read_at IS NULL \
          GROUP BY room_id \
          HAVING n > 0",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<i64, _>("room_id"), r.get::<i64, _>("n")))
        .collect())
}

/// Drop every mention row for `message_id`, returning the user IDs that
/// previously had unread mentions there. Used when a message is soft-
/// deleted: the caller fans out `MentionCleared` to those users so their
/// counts decrement, then this function removes the rows.
pub async fn delete_mentions_for_message(
    pool: &SqlitePool,
    message_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT mentioned_user_id FROM mentions WHERE message_id = ? AND read_at IS NULL",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await?;
    let users: Vec<String> = rows
        .into_iter()
        .map(|r| r.get::<String, _>("mentioned_user_id"))
        .collect();
    sqlx::query("DELETE FROM mentions WHERE message_id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(users)
}
