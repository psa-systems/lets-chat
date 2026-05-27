use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One feed row on the room's feeds page (no token material).
pub struct FeedRowView {
    pub id: i64,
    /// "rss" or "ical".
    pub kind: String,
    pub created_at: String,
    pub last_fetched_at: Option<String>,
    pub revoked: bool,
}

#[derive(Template)]
#[template(path = "room/feeds.html")]
pub struct RoomFeedsPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    /// True when the server has a secret key (required to hash feed tokens).
    /// When false, creation is disabled.
    pub available: bool,
    pub feeds: &'a [FeedRowView],
    /// Full feed URL for a just-created feed, shown exactly once.
    pub new_url: Option<String>,
    pub error: Option<String>,
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
