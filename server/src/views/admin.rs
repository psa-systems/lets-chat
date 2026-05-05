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
    pub section: &'static str,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_user: String,
    pub smtp_from: String,
    pub saved: bool,
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
    pub section: &'static str,
    pub invites: &'a [AdminInviteView],
}
