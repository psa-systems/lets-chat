use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row on the Saved page. Pre-resolved by the route handler so the
/// template stays free of business logic. `context_path` is the URL the
/// "in #room" / "in @peer" link points to.
pub struct SavedListRow {
    pub message_id: i64,
    pub author_label: String,
    pub body: String,
    pub message_created_at: String,
    pub saved_at: String,
    pub context_label: String,
    pub context_path: String,
}

#[derive(Template)]
#[template(path = "saved/page.html")]
pub struct SavedPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub entries: &'a [SavedListRow],
    pub asset_version: &'a str,
}
