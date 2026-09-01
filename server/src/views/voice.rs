#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template; // LC-188: in-scope for the |t/|tn template filters.

use crate::models::{Room, User};
use crate::views::layout::{SidebarPeer, SidebarRoom, SwitcherEntry};

/// One participant currently in a voice channel, as seen on the initial
/// server render. The browser mesh keeps the live grid in sync after the
/// viewer joins; this list is just the "who's here right now" preview.
pub struct VoiceParticipant {
    pub user_id: String,
    pub label: String,
}

#[derive(Template)]
#[template(path = "voice/page.html")]
pub struct VoicePage<'a> {
    pub user: &'a User,
    pub room: &'a Room,
    pub enclave_id: i64,
    pub sidebar_categories: &'a [crate::views::layout::SidebarCategoryGroup],
    pub sidebar_starred_rooms: &'a [SidebarRoom],
    pub sidebar_starred_peers: &'a [SidebarPeer],
    pub can_manage_sidebar_categories: bool,
    pub sidebar_current_enclave: Option<i64>,
    pub sidebar_rooms: &'a [SidebarRoom],
    pub sidebar_peers: &'a [SidebarPeer],
    pub switcher: &'a [SwitcherEntry],
    pub asset_version: &'a str,
    /// JSON array of `RTCIceServer` objects for the mesh peer connections.
    pub ice_servers: &'a str,
    pub participants: &'a [VoiceParticipant],
    /// LC-853: workspace remote-control switch. Gates the "Request control"
    /// affordance + consent UI include; the server refuses signals
    /// independently, so this is presentation, not enforcement.
    pub remote_control_enabled: bool,
}
