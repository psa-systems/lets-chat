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
    /// One of "starttls" | "tls" | "none". Drives the `<select>` rendering.
    pub smtp_tls_mode: String,
    /// Operator-supplied externally-reachable URL of this server. Used
    /// by the email digest to construct deep links back into the app.
    /// Empty string when not yet configured; in that case the digest
    /// renders without anchor tags (see templates/email/digest.{html,txt})
    /// and the admin page shows `base_url_missing_banner`.
    pub public_base_url: String,
    /// Whether newly-registered users start with the email-digest
    /// preference flipped on. Stored as a string "0" / "1" in the
    /// `settings` table under `default_notify_email_digest`; the admin
    /// toggle posts to a dedicated endpoint that flips it.
    pub default_notify_email_digest: bool,
    pub saved: bool,
    /// Banner copy explaining why email is currently disabled. None when
    /// email is fully operational (key + SMTP both present and decryptable).
    pub disabled_banner: Option<&'static str>,
    /// Populated by the "Send test email" POST handler. None on a plain GET.
    pub test_result: Option<TestEmailResult>,
}

/// Outcome of an admin test-email send. Rendered as a green-or-red banner
/// at the top of the SMTP form so the operator sees the SMTP error verbatim
/// instead of an opaque HTTP status code.
pub struct TestEmailResult {
    pub ok: bool,
    pub message: String,
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
}

#[derive(Template)]
#[template(path = "admin/enclaves.html")]
pub struct EnclavesPage<'a> {
    pub user: &'a User,
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
