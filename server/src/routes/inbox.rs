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
use crate::views::inbox::{render_item, InboxItem, InboxItemsFragment, InboxPage};
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
    let rows = db::inbox::list_unread(&state.chat, &user.id, is_admin, PAGE_SIZE, q.before).await?;

    let mut items: Vec<InboxItem> = Vec::with_capacity(rows.len());
    for row in &rows {
        let author = db::auth::find_user_by_id(&state.auth, &row.author_user_id).await?;
        let author_label = author
            .as_ref()
            .map(|r| {
                r.display_name
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| format!("@{}", r.username))
            })
            .unwrap_or_else(|| "(unknown)".to_string());
        // For DMs, derive the peer user id so the deep-link points
        // at /dm/{peer_id} rather than /room/{dm_room_id}.
        let peer_id = if row.room_type == "dm" {
            db::chat::get_dm_peer(&state.chat, row.room_id, &user.id).await?
        } else {
            None
        };
        items.push(render_item(row, author_label, peer_id.as_deref()));
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
