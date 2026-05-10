use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};
use crate::views::room::MessageView;

#[derive(Template)]
#[template(path = "dm/page.html")]
pub struct DmPage<'a> {
    pub user: &'a User,
    pub peer: &'a User,
    pub room: &'a Room,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
    /// "none" | "all". Drives the `Mute` checkbox `checked` state in the
    /// DM header partial.
    pub mute_mode: String,
}
