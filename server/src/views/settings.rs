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
    /// Mirror of `state.mail_available()`. Drives the disabled state of
    /// the email-digest checkbox and the help-text branch that explains
    /// why opting in is currently a no-op.
    pub email_available: bool,
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
    /// Enabled SSO providers the user could link to. Empty when no
    /// providers configured: the Linked Accounts card hides entirely.
    pub sso_providers: Vec<SettingsSsoProviderOption>,
    /// The user's own current sso_identities rows. Typically zero or
    /// one in v1; the schema is N-per-user ready.
    pub sso_identities: Vec<SettingsSsoIdentity>,
    /// True when the user has a non-NULL `password_hash`. Drives the
    /// Unlink button's disabled state (refusing to remove a user's
    /// only credential).
    pub has_password: bool,
    /// Flash set after a successful link / unlink action so the user
    /// sees confirmation when bounced back.
    pub sso_flash: Option<&'a str>,
}

pub struct SettingsSsoProviderOption {
    pub id: String,
    pub display_name: String,
    pub issuer_url: String,
    /// True when the user already has an identity row pointing at this
    /// provider's issuer; the per-provider Link button is hidden in
    /// that case (the Unlink button in the identities list handles it).
    pub already_linked: bool,
}

pub struct SettingsSsoIdentity {
    /// Display name of the provider the identity belongs to. Resolved
    /// server-side from the cached SsoProviders so the template doesn't
    /// have to thread the join.
    pub provider_display_name: String,
    pub issuer: String,
    pub email: Option<String>,
    pub auto_linked: bool,
    pub linked_at: String,
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
