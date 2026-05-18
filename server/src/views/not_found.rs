use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

#[derive(Template)]
#[template(path = "not_found.html")]
pub struct NotFoundPage<'a> {
    pub user: &'a User,
    pub path: Option<String>,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}
