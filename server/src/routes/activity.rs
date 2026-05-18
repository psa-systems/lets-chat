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

    let raw = db::activity::feed_for_user(&state.chat, &user.id, tab_filter, PAGE_LIMIT).await?;

    let mut items: Vec<ActivityItem> = Vec::with_capacity(raw.len());
    for item in raw {
        let actor = db::auth::find_user_by_id(&state.auth, &item.actor_user_id).await?;
        let actor_label = actor
            .as_ref()
            .and_then(|r| r.display_name.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| actor.as_ref().map(|r| format!("@{}", r.username)))
            .unwrap_or_else(|| "(unknown)".to_string());
        let room = db::chat::get_room(&state.chat, item.room_id).await?;
        let (room_label, target_path) = match room.as_ref() {
            Some(r) if r.room_type == "dm" => {
                let peer = db::chat::get_dm_peer(&state.chat, r.id, &user.id).await?;
                match peer {
                    Some(p) => (
                        format!("DM with @{p}"),
                        format!("/dm/{p}#msg-{}", item.message_id),
                    ),
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
