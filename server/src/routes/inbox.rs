//! LC-81: unread-message inbox.
//!
//! `GET /inbox` returns the full page on first hit; the infinite-
//! scroll sentinel re-fires with `?before={message_id}&fragment=1`
//! to swap in subsequent pages without re-rendering the chrome.
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::inbox::{render_item, InboxAuthor, InboxItem, InboxItemsFragment, InboxPage};
use crate::views::{html, Html};

const PAGE_SIZE: i64 = 30;

#[derive(Deserialize)]
pub struct InboxQuery {
    /// Cursor: only return messages with `id < before`. Omitted on
    /// the first page.
    #[serde(default)]
    pub before: Option<i64>,
    /// HTMX swap requests pass `fragment=1` so the handler renders
    /// only the new <li>s instead of the full page.
    #[serde(default)]
    pub fragment: Option<i64>,
}

pub async fn get_inbox(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<InboxQuery>,
) -> Result<Html, AppError> {
    let is_admin = user.role == "admin";
    let blocked = db::auth::list_blocked_ids_either_way(&state.auth, &user.id).await?;
    let rows = db::inbox::list_unread(
        &state.chat,
        &user.id,
        is_admin,
        &blocked,
        PAGE_SIZE,
        q.before,
    )
    .await?;

    // LC-685: the codebase-wide label rule (display name, else `@username`),
    // shared by the author and the DM peer below.
    let label_of = |r: &crate::models::user::UserRecord| -> String {
        r.display_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("@{}", r.username))
    };

    // LC-782: resolve every row's author and DM peer up front in a fixed
    // number of queries instead of one (or two) per row.
    let dm_room_ids: Vec<i64> = rows
        .iter()
        .filter(|r| r.room_type == "dm")
        .map(|r| r.room_id)
        .collect();
    let dm_peers = db::chat::dm_peers_for_rooms(&state.chat, &user.id, &dm_room_ids).await?;
    let user_ids: Vec<&str> = rows
        .iter()
        .map(|r| r.author_user_id.as_str())
        .chain(dm_peers.values().map(|s| s.as_str()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let users = db::auth::users_by_ids(&state.auth, &user_ids).await?;

    let mut items: Vec<InboxItem> = Vec::with_capacity(rows.len());
    for row in &rows {
        let author_rec = users.get(&row.author_user_id);
        // LC-685: carry the author's avatar + presence onto the row so it shows
        // an avatar like every other surface (previously fetched then discarded).
        let author = InboxAuthor {
            user_id: row.author_user_id.clone(),
            label: author_rec
                .map(&label_of)
                .unwrap_or_else(|| "(unknown)".to_string()),
            avatar_ext: author_rec.and_then(|r| r.avatar_ext.clone()),
            status: super::effective_status(
                &state,
                &row.author_user_id,
                author_rec.map(|r| r.status.as_str()).unwrap_or("offline"),
            ),
            custom_status: author_rec.and_then(|r| r.custom_status.clone()),
        };
        // For DMs, derive the peer user id (deep-link target) AND resolve its
        // display label so the caption reads a name, not a raw UUID (LC-685).
        let (peer_id, peer_label) = if row.room_type == "dm" {
            match dm_peers.get(&row.room_id) {
                Some(pid) => {
                    let plabel = users
                        .get(pid)
                        .map(&label_of)
                        .unwrap_or_else(|| format!("@{pid}"));
                    (Some(pid.clone()), Some(plabel))
                }
                None => (None, None),
            }
        } else {
            (None, None)
        };
        items.push(render_item(
            row,
            author,
            peer_id.as_deref(),
            peer_label.as_deref(),
        ));
    }

    // Cursor for the next page: the smallest message_id in this
    // page. Only set when we filled the requested page (more may
    // exist beyond).
    let next_cursor = if rows.len() as i64 == PAGE_SIZE {
        rows.last().map(|r| r.message_id)
    } else {
        None
    };

    if q.fragment.is_some() {
        let frag = InboxItemsFragment {
            items: &items,
            next_cursor,
        };
        return html(&frag);
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
    let page = InboxPage {
        user: &user,
        items: &items,
        next_cursor,
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
