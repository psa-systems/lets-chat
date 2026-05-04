use askama::Template;

use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom};

pub struct MessageView {
    pub id: i64,
    pub user_id: String,
    pub username: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub body: String,
    pub reactions: Vec<ReactionView>,
    pub can_edit: bool,
    pub can_delete: bool,
    pub viewer_id: String,
}

pub struct ReactionView {
    pub emoji: String,
    pub count: i64,
    pub viewer_reacted: bool,
}

#[derive(Template)]
#[template(path = "room/page.html")]
pub struct RoomPage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub messages: &'a [MessageView],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "room/composer.html")]
pub struct ComposerFragment<'a> {
    pub room: &'a Room,
}

#[derive(Template)]
#[template(path = "room/edit_form.html")]
pub struct EditFormFragment<'a> {
    pub message_id: i64,
    pub current_body: &'a str,
}

#[derive(Template)]
#[template(path = "room/message.html")]
pub struct SingleMessageFragment<'a> {
    pub message: &'a MessageView,
    pub oob: bool,
}

#[derive(Template)]
#[template(path = "partials/reaction_bar.html")]
pub struct ReactionBarFragment<'a> {
    pub message_id: i64,
    pub reactions: &'a [ReactionView],
}
