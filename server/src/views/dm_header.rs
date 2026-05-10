use askama::Template;

use crate::models::{Room, User};

#[derive(Template)]
#[template(path = "partials/dm_header.html")]
pub struct DmHeaderFragment<'a> {
    pub peer: &'a User,
    pub room: &'a Room,
    pub mute_mode: &'a str,
}
