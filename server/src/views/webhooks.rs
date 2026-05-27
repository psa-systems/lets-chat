#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One webhook row on the room's webhook page (no secret material).
pub struct WebhookRowView {
    pub id: i64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

#[derive(Template)]
#[template(path = "room/webhooks.html")]
pub struct RoomWebhooksPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    /// True when the server has a secret key (required to hash webhook
    /// secrets). When false, creation is disabled.
    pub available: bool,
    pub webhooks: &'a [WebhookRowView],
    /// Full webhook URL for a just-created hook, shown exactly once.
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
