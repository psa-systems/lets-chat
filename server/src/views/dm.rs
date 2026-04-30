use askama::Template;

use crate::models::{Room, User};
use crate::views::room::MessageView;

#[derive(Template)]
#[template(path = "dm/page.html")]
pub struct DmPage<'a> {
    pub user: &'a User,
    pub peer: &'a User,
    pub room: &'a Room,
    pub rooms: &'a [Room],
    pub dm_peers: &'a [User],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
}
