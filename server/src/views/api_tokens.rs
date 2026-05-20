use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One token row on the management page (no secret material).
pub struct ApiTokenRowView {
    pub id: i64,
    pub name: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked: bool,
}

#[derive(Template)]
#[template(path = "settings/api_tokens.html")]
pub struct ApiTokensPage<'a> {
    pub user: &'a User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    /// True when the deployment has a secret key configured (API tokens
    /// require it to key the HMAC). When false, minting is disabled.
    pub available: bool,
    pub all_scopes: &'a [&'a str],
    pub tokens: &'a [ApiTokenRowView],
    /// The plaintext of a just-created token, shown exactly once.
    pub new_token: Option<String>,
    pub error: Option<String>,
}
