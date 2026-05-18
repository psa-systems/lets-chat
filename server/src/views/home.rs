use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub flash_error: Option<&'a str>,
}
