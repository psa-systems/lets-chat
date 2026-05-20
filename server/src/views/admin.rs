use askama::Template;

use crate::models::{ModAction, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// Per-row projection for the users admin table.
pub struct AdminUserView {
    pub id: String,
    pub username: String,
    pub role: String,
    pub is_banned: bool,
    pub is_muted: bool,
    /// LC-93: pre-formatted "N.NN MiB" string of the user's current
    /// upload usage (excludes system messages).
    pub usage_display: String,
    /// LC-93: current quota as a whole-MiB string for the form input,
    /// or empty when the user is unlimited. An empty submit clears the
    /// quota.
    pub quota_mib_value: String,
}

/// Per-row projection for the invites admin table.
pub struct AdminInviteView {
    pub id: i64,
    pub code: String,
    pub created_by_username: String,
    pub used_by_username: Option<String>,
    pub created_at: String,
}

/// Per-row projection for the rooms admin table.
pub struct AdminRoomView {
    pub id: i64,
    pub name: String,
    pub topic: Option<String>,
    pub room_type: String,
    pub invite_code: Option<String>,
    pub members: i64,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/room_row.html")]
pub struct RoomRowFragment<'a> {
    pub r: &'a AdminRoomView,
}

#[derive(Template)]
#[template(path = "admin/users.html")]
pub struct UsersPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub users: &'a [AdminUserView],
}

#[derive(Template)]
#[template(path = "admin/user_row.html")]
pub struct UserRowFragment<'a> {
    pub u: &'a AdminUserView,
}

#[derive(Template)]
#[template(path = "admin/rooms.html")]
pub struct RoomsPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rooms_admin: &'a [AdminRoomView],
}

#[derive(Template)]
#[template(path = "admin/modlog.html")]
pub struct ModLogPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub entries: &'a [ModAction],
}

#[derive(Template)]
#[template(path = "admin/settings.html")]
pub struct SettingsPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_user: String,
    pub smtp_from: String,
    /// Whether newly-registered users start with the email-digest
    /// preference flipped on. Stored as a string "0" / "1" in the
    /// `settings` table under `default_notify_email_digest`.
    pub default_notify_email_digest: bool,
    /// Current value of the maintenance-mode toggle.
    pub maintenance_enabled: bool,
    /// Operator-facing message shown on the 503 page while maintenance
    /// is on. Empty hides the message block.
    pub maintenance_message: String,
    pub saved: bool,
    /// Pre-formatted "N.NN MiB" string; Askama can't do the i64-to-f64
    /// arithmetic inline, so the handler renders it.
    pub uploads_total_display: String,
    pub uploads_orphan_count: i64,
    /// Flash counts populated from the post-redirect query string.
    /// Some(N) renders a one-line success banner; None renders nothing.
    pub regenerated: Option<i64>,
    pub purged: Option<i64>,
}

pub struct AdminEnclaveView {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub invite_code: Option<String>,
    pub member_count: i64,
    pub owner_id: Option<String>,
    pub created_at: String,
    /// LC-93: same formatting convention as `AdminUserView`.
    pub usage_display: String,
    pub quota_mib_value: String,
}

#[derive(Template)]
#[template(path = "admin/enclaves.html")]
pub struct EnclavesPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub enclaves: &'a [AdminEnclaveView],
}

#[derive(Template)]
#[template(path = "admin/invites.html")]
pub struct InvitesPage<'a> {
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
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub invites: &'a [AdminInviteView],
}

// LC-94 anti-spam pages ----------------------------------------------------

#[derive(Template)]
#[template(path = "admin/anti_spam.html")]
pub struct AntiSpamPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rate_limit_messages: u32,
    pub rate_limit_registrations: u32,
    pub rate_limit_password_resets: u32,
    pub link_filter_enabled: bool,
    pub honeypot_enabled: bool,
    pub saved: bool,
}

pub struct LinkFilterRuleView {
    pub id: i64,
    pub pattern: String,
    pub action: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/link_filter.html")]
pub struct LinkFilterPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub rules: &'a [LinkFilterRuleView],
    pub error: Option<String>,
}

pub struct QuarantineEntryView {
    pub message_id: i64,
    pub room_id: i64,
    pub author_id: String,
    pub body: String,
    pub matched_pattern: String,
    pub matched_url: String,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin/quarantine.html")]
pub struct QuarantinePage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub entries: &'a [QuarantineEntryView],
}

// LC-95 backup / restore ----------------------------------------------------

#[derive(Template)]
#[template(path = "admin/backup_restore.html")]
pub struct BackupRestorePage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    /// True when a `.restore-pending` marker exists in the data dir;
    /// the page shows a banner instructing the operator to restart.
    pub restore_pending: bool,
    /// Inline error from a failed stage attempt; rendered above the
    /// upload form on the retry.
    pub error: Option<String>,
}

// LC-96 branding admin page -----------------------------------------------

#[derive(Template)]
#[template(path = "admin/branding.html")]
pub struct BrandingPage<'a> {
    pub user: &'a crate::models::User,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    pub app_version: &'a str,
    pub git_hash: &'a str,
    pub git_version: &'a str,
    pub build_date: &'a str,
    pub section: &'static str,
    pub primary_color: String,
    pub accent_color: String,
    pub login_heading: String,
    pub login_body: String,
    pub has_logo: bool,
    pub saved: bool,
    pub error: Option<String>,
}
