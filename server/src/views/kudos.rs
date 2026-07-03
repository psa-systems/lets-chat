//! LC-526: kudos leaderboard page.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

/// One ranked leaderboard row.
pub struct LeaderRow {
    pub rank: usize,
    pub label: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "kudos/page.html")]
pub struct KudosPage<'a> {
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
    /// Most-appreciated (top receivers).
    pub receivers: Vec<LeaderRow>,
    /// Most-generous (top givers).
    pub givers: Vec<LeaderRow>,
}
