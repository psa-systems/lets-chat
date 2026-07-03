//! LC-529: reaction highlights recap page (per room).

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One highlighted message row. `snippet` is a plain-text, length-capped
/// excerpt (the recap is a lightweight overview, not the full thread).
pub struct HighlightRow {
    pub message_id: i64,
    pub author_label: String,
    pub created_at: String,
    pub snippet: String,
    pub total: i64,
    /// `(emoji, count)` chips, most-reacted first.
    pub emojis: Vec<(String, i64)>,
}

#[derive(Template)]
#[template(path = "room/highlights.html")]
pub struct HighlightsPage<'a> {
    pub user: &'a User,
    pub asset_version: &'a str,
    pub sidebar_categories: &'a [SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub room_label: String,
    pub back_path: String,
    pub rows: Vec<HighlightRow>,
}
