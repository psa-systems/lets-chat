#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// LC-470: public marketing landing page shown at `/` to logged-out visitors
/// (authenticated users go straight to the chat home). Mirrors the Bunyip /
/// Mokosh landing pattern: hero with the messenger-bird mascot, a features
/// grid, and a "Sign in with Bunyip" call to action.
#[derive(Template)]
#[template(path = "landing.html")]
pub struct LandingPage<'a> {
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub build_date: &'a str,
    /// Host portion of the configured Bunyip issuer, shown in the
    /// "you'll be redirected to ..." note (same as the login page).
    pub bunyip_issuer_host: &'a str,
}

#[derive(Template)]
#[template(path = "home/welcome.html")]
pub struct WelcomePage<'a> {
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
    pub flash_error: Option<&'a str>,
    /// LC-372: count of pending enclave invitations, drives the count badge on
    /// the welcome empty-state "View invitations" quick action (hidden when 0).
    pub pending_invites: usize,
    /// LC-516: false when the user belongs to no enclaves; drives the inline
    /// "create your first enclave" prompt that replaced the auto-default enclave.
    pub has_enclaves: bool,
}
