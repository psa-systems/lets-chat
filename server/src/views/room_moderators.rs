use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row in the existing-overrides table.
pub struct RoomOverrideEntry {
    pub user_id: String,
    pub user_label: String,
    pub role: String,
    pub assigned_by_label: String,
    pub assigned_at: String,
}

/// One option in the grant <select> (room-enclave member who does NOT
/// already have an override).
pub struct RoomModeratorRow {
    pub user_id: String,
    pub label: String,
}

#[derive(Template)]
#[template(path = "room/moderators.html")]
pub struct RoomModeratorsPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub overrides: &'a [RoomOverrideEntry],
    pub candidates: &'a [RoomModeratorRow],
    /// LC-85: current `posting_allowed_for` for this room. Drives the
    /// "Posting policy" dropdown's selected option.
    pub posting_policy: &'a str,
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
