#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

#[derive(Template)]
#[template(path = "maintenance.html")]
pub struct MaintenancePage<'a> {
    pub message: &'a str,
    pub asset_version: &'a str,
}
