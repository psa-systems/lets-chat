//! Activity feed (LC-82).
//!
//! On-demand UNION over the existing `mentions`, `message_reactions`,
//! and `messages` (for replies) tables. No new tables; ship the cheap
//! write path first, convert to materialized later if reads get
//! expensive.
//!
//! Each row is normalized to `ActivityItem { kind, message_id,
//! room_id, actor_user_id, created_at }`. Callers join against
//! `auth.db` separately to resolve display names; this module stays
//! confined to chat.db reads.
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Mention,
    Reply,
    Reaction,
}

impl ActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ActivityKind::Mention => "mention",
            ActivityKind::Reply => "reply",
            ActivityKind::Reaction => "reaction",
        }
    }
}

pub struct ActivityItem {
    pub kind: ActivityKind,
    /// Source message (mention's host message; reply's reply; reaction's
    /// target message).
    pub message_id: i64,
    pub room_id: i64,
    /// Who did the thing. For mentions / replies / reactions this is
    /// the mentioner / replier / reactor.
    pub actor_user_id: String,
    /// For reactions, the emoji shortcode; empty for other kinds.
    pub emoji: String,
    pub created_at: String,
}

/// Fetch the user's activity feed filtered by `tab`. `Mention` /
/// `Reply` / `Reaction` filter to one kind; passing `None` returns
/// all three merged time-DESC. Bounded by `limit` per kind so a busy
/// room doesn't drown out the other two.
pub async fn feed_for_user(
    pool: &SqlitePool,
    user_id: &str,
    is_admin: bool,
    tab: Option<ActivityKind>,
    limit: i64,
) -> Result<Vec<ActivityItem>, sqlx::Error> {
    // LC-606: all three queries used `room_type = 'public' OR member`, with no
    // enclave condition, so an item could surface from a room the viewer would
    // be refused (someone in another enclave @mentions you, or you were removed
    // from an enclave after posting and keep receiving replies/reactions).
    // Visibility now comes from the shared predicate that mirrors
    // `is_room_accessible`, the same fix LC-604 applied to the unread queries.
    let access = crate::db::chat::accessible_rooms_sql(is_admin);
    let access_binds = crate::db::chat::accessible_rooms_binds(is_admin);
    let mut out: Vec<ActivityItem> = Vec::new();

    if matches!(tab, None | Some(ActivityKind::Mention)) {
        let sql = format!(
            "SELECT m.id AS message_id, m.room_id, m.user_id AS actor_user_id, m.created_at \
               FROM mentions men \
               INNER JOIN messages m ON m.id = men.message_id \
               INNER JOIN rooms r ON r.id = m.room_id \
              WHERE men.mentioned_user_id = ? \
                AND m.deleted_at IS NULL AND m.quarantined = 0 \
                AND m.user_id != ? \
                AND {access} \
              ORDER BY m.created_at DESC, m.id DESC \
              LIMIT ?",
            access = access,
        );
        let mut q = sqlx::query(&sql).bind(user_id).bind(user_id);
        for _ in 0..access_binds {
            q = q.bind(user_id);
        }
        let rows = q.bind(limit).fetch_all(pool).await?;
        for r in rows {
            out.push(ActivityItem {
                kind: ActivityKind::Mention,
                message_id: r.get("message_id"),
                room_id: r.get("room_id"),
                actor_user_id: r.get("actor_user_id"),
                emoji: String::new(),
                created_at: r.get("created_at"),
            });
        }
    }

    if matches!(tab, None | Some(ActivityKind::Reply)) {
        let sql = format!(
            "SELECT m.id AS message_id, m.room_id, m.user_id AS actor_user_id, m.created_at \
               FROM messages m \
               INNER JOIN messages parent ON parent.id = m.parent_id \
               INNER JOIN rooms r ON r.id = m.room_id \
              WHERE parent.user_id = ? \
                AND m.user_id != ? \
                AND m.deleted_at IS NULL AND m.quarantined = 0 \
                AND parent.deleted_at IS NULL \
                AND {access} \
              ORDER BY m.created_at DESC, m.id DESC \
              LIMIT ?",
            access = access,
        );
        let mut q = sqlx::query(&sql).bind(user_id).bind(user_id);
        for _ in 0..access_binds {
            q = q.bind(user_id);
        }
        let rows = q.bind(limit).fetch_all(pool).await?;
        for r in rows {
            out.push(ActivityItem {
                kind: ActivityKind::Reply,
                message_id: r.get("message_id"),
                room_id: r.get("room_id"),
                actor_user_id: r.get("actor_user_id"),
                emoji: String::new(),
                created_at: r.get("created_at"),
            });
        }
    }

    if matches!(tab, None | Some(ActivityKind::Reaction)) {
        let sql = format!(
            "SELECT m.id AS message_id, m.room_id, mr.user_id AS actor_user_id, \
                    mr.emoji, mr.created_at \
               FROM message_reactions mr \
               INNER JOIN messages m ON m.id = mr.message_id \
               INNER JOIN rooms r ON r.id = m.room_id \
              WHERE m.user_id = ? \
                AND mr.user_id != ? \
                AND m.deleted_at IS NULL AND m.quarantined = 0 \
                AND {access} \
              ORDER BY mr.created_at DESC \
              LIMIT ?",
            access = access,
        );
        let mut q = sqlx::query(&sql).bind(user_id).bind(user_id);
        for _ in 0..access_binds {
            q = q.bind(user_id);
        }
        let rows = q.bind(limit).fetch_all(pool).await?;
        for r in rows {
            out.push(ActivityItem {
                kind: ActivityKind::Reaction,
                message_id: r.get("message_id"),
                room_id: r.get("room_id"),
                actor_user_id: r.get("actor_user_id"),
                emoji: r.get("emoji"),
                created_at: r.get("created_at"),
            });
        }
    }

    // Merge-sort newest-first across kinds. Stable sort keeps tied
    // timestamps in (mention, reply, reaction) priority order, which
    // matches the typical importance ordering.
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out.truncate(limit as usize);
    Ok(out)
}
