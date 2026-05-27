use askama::Template;

#[allow(unused_imports)]
use crate::i18n::filters; // LC-100: in-scope for the `| t` filter in templates.
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
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
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
    /// LC-93: pre-formatted "N.NN MiB" of the user's current upload
    /// usage. Always populated; an unlimited account renders the
    /// usage with an "(unlimited)" tail label.
    pub storage_usage_display: String,
    /// LC-93: `Some("N.NN MiB")` when the user has a cap, `None` when
    /// they are unlimited.
    pub storage_quota_display: Option<String>,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    /// LC-88: Do Not Disturb. `dnd_active` drives the "currently quiet"
    /// banner; `dnd_paused_until` is the raw stored instant (empty when no
    /// manual pause). The five schedule fields are pre-split from the stored
    /// JSON so the form renders the user's current quiet hours. `timezones`
    /// is the IANA name list for the timezone picker.
    pub dnd_active: bool,
    pub dnd_paused_until: String,
    pub dnd_timezone: String,
    pub dnd_weekday_start: String,
    pub dnd_weekday_end: String,
    pub dnd_weekend_start: String,
    pub dnd_weekend_end: String,
    pub timezones: Vec<TzOption>,
    /// LC-100: available UI locales for the language picker, plus a synthetic
    /// "system" option (empty code) for the Accept-Language fallback.
    pub locales: Vec<LocaleOption>,
}

/// One entry in the language picker. `selected` is precomputed; an empty
/// `code` is the "use browser language" (clear preference) option.
pub struct LocaleOption {
    pub code: String,
    pub name: String,
    pub selected: bool,
}

/// One entry in the DND timezone picker. `selected` is precomputed so the
/// template needs no string comparison (Askama can't compare `String` to the
/// `&&str` yielded by iterating a `&[&str]`).
pub struct TzOption {
    pub name: &'static str,
    pub selected: bool,
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
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub blocked: &'a [BlockedUserView],
    pub error: Option<&'a str>,
    pub form_username: &'a str,
}
