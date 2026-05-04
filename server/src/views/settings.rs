use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom};

#[derive(Template)]
#[template(path = "settings/page.html")]
pub struct UserSettingsPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub asset_version: &'a str,
    pub saved: bool,
}
