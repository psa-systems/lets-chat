use askama::Template;

use crate::models::{Room, User};

pub struct MessageView {
    pub id: i64,
    pub username: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
}

pub struct ReactionView {
    pub emoji: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "room/page.html")]
pub struct RoomPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub rooms: &'a [Room],
    pub dm_peers: &'a [User],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "room/composer.html")]
pub struct ComposerFragment<'a> {
    pub room: &'a Room,
}
