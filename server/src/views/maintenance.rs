use askama::Template;

#[derive(Template)]
#[template(path = "maintenance.html")]
pub struct MaintenancePage<'a> {
    pub message: &'a str,
    pub asset_version: &'a str,
}
