#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::db::inbox::InboxRow;
use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One message in the inbox. Renders inside a per-room group.
pub struct InboxItem {
    pub message_id: i64,
    pub room_id: i64,
    pub room_label: String,
    pub author_label: String,
    /// LC-685: author identity for the row avatar (`partials/avatar.html`).
    pub author_user_id: String,
    pub avatar_ext: Option<String>,
    /// Effective presence (`online`/`idle`/`dnd`/`offline`) for the avatar dot,
    /// resolved via `routes::effective_status` like every other avatar surface.
    pub author_status: String,
    pub author_custom_status: Option<String>,
    pub snippet: String,
    pub created_at: String,
    /// Deep-link target: /room/{id}#msg-{message_id} for channels,
    /// /dm/{peer_id}#msg-{message_id} for DMs.
    pub target_path: String,
}

#[derive(Template)]
#[template(path = "inbox/page.html")]
pub struct InboxPage<'a> {
    pub user: &'a User,
    pub items: &'a [InboxItem],
    /// Cursor for the next page (last item's message_id) or None when
    /// the current page is the last.
    pub next_cursor: Option<i64>,
    pub sidebar_categories: &'a [SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

/// HTMX fragment that swaps in the next page of inbox items. Used by
/// the infinite-scroll sentinel at the bottom of the list; the
/// initial page returns the full `InboxPage`.
#[derive(Template)]
#[template(path = "inbox/items.html")]
pub struct InboxItemsFragment<'a> {
    pub items: &'a [InboxItem],
    pub next_cursor: Option<i64>,
}

/// Author identity resolved by the caller (it crosses the auth.db / chat.db
/// split), passed into [`render_item`] so the view can render the row avatar.
pub struct InboxAuthor {
    pub user_id: String,
    pub label: String,
    pub avatar_ext: Option<String>,
    /// Effective presence (already run through `routes::effective_status`).
    pub status: String,
    pub custom_status: Option<String>,
}

/// Convert a raw `InboxRow` (from the DB layer) into the renderable
/// `InboxItem`. Author identity + the DM peer's display label are the caller's
/// job (they cross the auth.db / chat.db split); this function just formats the
/// bits the view layer can handle on its own. `peer_id` drives the DM deep-link,
/// `peer_label` the caption (LC-685: previously the raw peer id leaked into the
/// caption as "DM with @<uuid>").
pub fn render_item(
    row: &InboxRow,
    author: InboxAuthor,
    peer_id: Option<&str>,
    peer_label: Option<&str>,
) -> InboxItem {
    let snippet: String = row
        .body
        .chars()
        .take(140)
        .collect::<String>()
        .replace('\n', " ");
    let room_label = match row.room_type.as_str() {
        "dm" => peer_label
            .map(|p| format!("DM with {p}"))
            .unwrap_or_else(|| "Direct message".to_string()),
        _ => format!("#{}", row.room_name),
    };
    let target_path = match row.room_type.as_str() {
        "dm" => peer_id
            .map(|p| format!("/dm/{p}#msg-{}", row.message_id))
            .unwrap_or_else(|| format!("/room/{}#msg-{}", row.room_id, row.message_id)),
        _ => format!("/room/{}#msg-{}", row.room_id, row.message_id),
    };
    InboxItem {
        message_id: row.message_id,
        room_id: row.room_id,
        room_label,
        author_label: author.label,
        author_user_id: author.user_id,
        avatar_ext: author.avatar_ext,
        author_status: author.status,
        author_custom_status: author.custom_status,
        snippet,
        created_at: row.created_at.clone(),
        target_path,
    }
}
