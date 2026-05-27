#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};

#[derive(Template)]
#[template(path = "partials/dm_header.html")]
pub struct DmHeaderFragment<'a> {
    pub peer: &'a User,
    pub room: &'a Room,
    pub mute_mode: &'a str,
}
