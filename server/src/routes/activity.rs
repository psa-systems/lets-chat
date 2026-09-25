//! LC-82: activity center.
//!
//! `GET /activity[?tab=all|mention|reply|reaction]` lists every event
//! affecting the calling user across rooms they can still see -
//! mentions of them, replies to messages they authored, reactions on
//! messages they authored. On-demand UNION over the existing
//! `mentions`, `messages`, and `message_reactions` tables; no new
//! tables. Per-item read state is a follow-up.
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::db::activity::ActivityKind;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::activity::{ActivityItem, ActivityPage};
use crate::views::{html, Html};

const PAGE_LIMIT: i64 = 60;

#[derive(Deserialize)]
pub struct ActivityQuery {
    #[serde(default)]
    pub tab: Option<String>,
}

pub async fn get_activity(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ActivityQuery>,
) -> Result<Html, AppError> {
    let tab_filter = match q.tab.as_deref() {
        Some("mention") => Some(ActivityKind::Mention),
        Some("reply") => Some(ActivityKind::Reply),
        Some("reaction") => Some(ActivityKind::Reaction),
        _ => None,
    };
    let active_tab = match tab_filter {
        Some(ActivityKind::Mention) => "mention",
        Some(ActivityKind::Reply) => "reply",
        Some(ActivityKind::Reaction) => "reaction",
        None => "all",
    };

    let blocked = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let raw = db::activity::feed_for_user(
        &state.chat,
        &user.id,
        user.role == "admin",
        &blocked,
        tab_filter,
        PAGE_LIMIT,
    )
    .await?;

    // LC-690: the codebase-wide label rule (display name, else `@username`),
    // shared by the actor and the DM peer below.
    let label_of = |r: &crate::models::user::UserRecord| -> String {
        r.display_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("@{}", r.username))
    };

    // LC-782: resolve every row's actor, room and DM peer up front in a fixed
    // number of queries instead of one (or more) per row.
    let room_ids: Vec<i64> = raw
        .iter()
        .map(|item| item.room_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let rooms = db::chat::rooms_by_ids(&state.chat, &room_ids).await?;
    let dm_room_ids: Vec<i64> = rooms
        .values()
        .filter(|r| r.room_type == "dm")
        .map(|r| r.id)
        .collect();
    let dm_peers = db::chat::dm_peers_for_rooms(&state.chat, &user.id, &dm_room_ids).await?;
    let user_ids: Vec<&str> = raw
        .iter()
        .map(|item| item.actor_user_id.as_str())
        .chain(dm_peers.values().map(|s| s.as_str()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let users = db::auth::users_by_ids(&state.auth, &user_ids).await?;

    let mut items: Vec<ActivityItem> = Vec::with_capacity(raw.len());
    for item in raw {
        let actor = users.get(&item.actor_user_id);
        // LC-690: carry the actor's avatar + presence onto the row so it shows an
        // avatar like the Inbox / timeline (previously fetched then discarded).
        let actor_label = actor
            .map(&label_of)
            .unwrap_or_else(|| "(unknown)".to_string());
        let avatar_ext = actor.and_then(|r| r.avatar_ext.clone());
        let actor_status = super::effective_status(
            &state,
            &item.actor_user_id,
            actor.map(|r| r.status.as_str()).unwrap_or("offline"),
        );
        let actor_custom_status = actor.and_then(|r| r.custom_status.clone());
        let room = rooms.get(&item.room_id);
        let (room_label, target_path) = match room {
            Some(r) if r.room_type == "dm" => {
                let peer = dm_peers.get(&r.id);
                match peer {
                    // LC-690: resolve the peer's display name for the caption, not
                    // the raw UUID; the id still drives the deep-link.
                    Some(p) => {
                        let peer_label = users
                            .get(p)
                            .map(&label_of)
                            .unwrap_or_else(|| format!("@{p}"));
                        (
                            format!("DM with {peer_label}"),
                            format!("/dm/{p}#msg-{}", item.message_id),
                        )
                    }
                    None => (
                        "Direct message".to_string(),
                        format!("/room/{}#msg-{}", r.id, item.message_id),
                    ),
                }
            }
            Some(r) => (
                format!("#{}", r.name),
                format!("/room/{}#msg-{}", r.id, item.message_id),
            ),
            None => (
                "(unknown)".to_string(),
                format!("/room/{}#msg-{}", item.room_id, item.message_id),
            ),
        };
        items.push(ActivityItem {
            kind: item.kind.as_str().to_string(),
            kind_label: match item.kind {
                ActivityKind::Mention => "Mention".into(),
                ActivityKind::Reply => "Reply".into(),
                ActivityKind::Reaction => "Reaction".into(),
            },
            message_id: item.message_id,
            room_id: item.room_id,
            room_label,
            actor_label,
            actor_user_id: item.actor_user_id,
            avatar_ext,
            actor_status,
            actor_custom_status,
            emoji: item.emoji,
            created_at: item.created_at,
            target_path,
        });
    }

    let (
        sidebar_categories,
        sidebar_starred_rooms,
        sidebar_starred_peers,
        sidebar_rooms,
        sidebar_peers,
        switcher,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
    ) = super::load_chrome(&state, &user, None).await?;
    let page = ActivityPage {
        user: &user,
        items: &items,
        active_tab,
        sidebar_categories: &sidebar_categories,
        sidebar_starred_rooms: &sidebar_starred_rooms,
        sidebar_starred_peers: &sidebar_starred_peers,
        can_manage_sidebar_categories,
        sidebar_current_enclave,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
    };
    html(&page)
}
