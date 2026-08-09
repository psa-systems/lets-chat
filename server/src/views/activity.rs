#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

pub struct ActivityItem {
    pub kind: String,
    pub kind_label: String,
    pub message_id: i64,
    pub room_id: i64,
    pub room_label: String,
    pub actor_label: String,
    /// LC-690: actor identity for the row avatar (`partials/avatar.html`).
    pub actor_user_id: String,
    pub avatar_ext: Option<String>,
    /// Effective presence (`online`/`idle`/`dnd`/`offline`) for the avatar dot,
    /// resolved via `routes::effective_status` like every other avatar surface.
    pub actor_status: String,
    pub actor_custom_status: Option<String>,
    /// For reactions, the emoji shortcode. Empty for other kinds.
    pub emoji: String,
    pub created_at: String,
    pub target_path: String,
}

#[derive(Template)]
#[template(path = "activity/page.html")]
pub struct ActivityPage<'a> {
    pub user: &'a User,
    pub items: &'a [ActivityItem],
    pub active_tab: &'a str,
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
