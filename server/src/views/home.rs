use askama::Template;

use crate::models::User;

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
    pub user: &'a User,
    pub asset_version: &'a str,
}
