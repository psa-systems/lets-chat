use askama::Template;

use crate::models::{Room, User};

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
    pub user: &'a User,
    pub rooms: &'a [Room],
    pub dm_peers: &'a [User],
    pub asset_version: &'a str,
}
