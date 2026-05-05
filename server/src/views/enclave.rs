use askama::Template;

use crate::models::enclave::{Enclave, EnclaveInvitation, EnclaveMembership};
use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

#[derive(Template)]
#[template(path = "enclave/page.html")]
pub struct EnclavePage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMembership],
    pub rooms: &'a [Room],
    pub can_manage: bool,
    pub flash_error: Option<&'a str>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "enclave/settings.html")]
pub struct EnclaveSettingsPage<'a> {
    pub user: &'a User,
    pub enclave: &'a Enclave,
    pub members: &'a [EnclaveMembership],
    pub can_delete: bool,
    pub flash_error: Option<&'a str>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "enclave/discover.html")]
pub struct DiscoverPage<'a> {
    pub user: &'a User,
    pub enclaves: &'a [Enclave],
    pub flash_error: Option<&'a str>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}

#[derive(Template)]
#[template(path = "invitations/page.html")]
pub struct InvitationsPage<'a> {
    pub user: &'a User,
    pub invitations: &'a [(EnclaveInvitation, Enclave)],
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
}
