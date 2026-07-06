//! LC-536: personal member-stats ("wrapped") page. A self-service recap of the
//! viewer's own activity; never shows another user's numbers.

#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::db::stats::TopRoom;
use crate::models::User;
use crate::views::layout::{SidebarCategoryGroup, SidebarPeer, SidebarRoom, SwitcherEntry};

#[derive(Template)]
#[template(path = "stats/page.html")]
pub struct StatsPage<'a> {
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
    /// Date portion of the user's join instant (e.g. "2026-01-15"), or empty.
    pub member_since: String,
    pub messages_sent: i64,
    pub active_days: i64,
    pub reactions_given: i64,
    pub reactions_received: i64,
    pub kudos_received: i64,
    pub top_rooms: Vec<TopRoom>,
}
