use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom};

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub asset_version: &'a str,
}
