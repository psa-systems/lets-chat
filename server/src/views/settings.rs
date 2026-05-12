use askama::Template;

use crate::models::User;
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One row in the sessions list on the settings page.
pub struct SessionView {
    pub id: String,
    pub label: String,
    pub ip: Option<String>,
    pub last_seen: String,
    pub created: String,
    pub is_current: bool,
}

#[derive(Template)]
#[template(path = "settings/page.html")]
pub struct UserSettingsPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub saved: bool,
    pub push_available: bool,
    pub email: Option<String>,
    /// True when the email on file has been verified by clicking the link
    /// sent to that address. Always false when `email` is `None`.
    pub email_verified: bool,
    /// True only when SMTP is configured *and* the build supports the
    /// in-tree verification flow (standalone). Drives whether the
    /// pending/verified badge and resend button appear at all.
    pub email_verification_available: bool,
    /// Flash set by the resend redirect: render a small "we sent a fresh
    /// link" notice next to the email field.
    pub email_verify_sent: bool,
    /// Hide the password-change form entirely in SaaS mode, where identity
    /// is owned by the parent app.
    pub password_change_available: bool,
    pub password_changed: bool,
    pub password_error: Option<&'a str>,
    /// Live sessions for this user, sorted newest activity first. The row
    /// matching the request's session cookie has `is_current = true` so the
    /// template can mark it and disable its revoke button.
    pub sessions: &'a [SessionView],
    /// Flash set by `?session_revoked=1` after a successful revoke.
    pub session_revoked: bool,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
}

pub struct BlockedUserView {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_ext: Option<String>,
}

impl BlockedUserView {
    pub fn label(&self) -> &str {
        match self.display_name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.username,
        }
    }
}

#[derive(Template)]
#[template(path = "settings/blocked.html")]
pub struct BlockedListPage<'a> {
    pub user: &'a User,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub blocked: &'a [BlockedUserView],
    pub error: Option<&'a str>,
    pub form_username: &'a str,
}
